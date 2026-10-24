//! Node data for the synthesizer graph.
//!
//! Defines the per-node data stored in the graph editor.
//!
//! # Custom Rendering
//!
//! This module implements custom node rendering to match the concept image aesthetic:
//! - Colored header bars based on module category
//! - Module icons in the header
//! - Horizontal knob row at the bottom for controllable parameters
//! - Category labels in the footer

use std::collections::BTreeMap;

use eframe::egui::{self, Color32, RichText};
use egui_node_graph2::{ConnectionSignalTrait, NodeDataTrait, NodeResponse, UserResponseTrait};

use crate::dsp::ModuleCategory;
use crate::widgets::{knob, led, KnobConfig, LedConfig};
use super::groups::{GroupId, NodeKind, Side};
use super::hints::{self, Hint};
use super::step_grid::step_sequencer_display;
use super::{SynthResponse, SynthValueType};

/// Describes how a knob parameter interacts with its CV input port.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum KnobInputMode {
    /// Knob only - no corresponding input port.
    #[default]
    KnobOnly,
    /// CV replaces knob value when connected. Knob becomes read-only and
    /// displays the incoming signal value.
    Exposed,
    /// CV modulates the knob's base value when connected. Knob remains
    /// interactive for setting the base/center value. Good for parameters
    /// like filter cutoff or oscillator frequency where you want CV to
    /// modulate around a user-set center point.
    Modulatable,
}

/// Describes a knob parameter that appears in the bottom section of a node.
///
/// Knob parameters provide manual control over module values. They can optionally
/// be "exposed" as input ports, allowing external signals to modulate or replace
/// the knob value.
#[derive(Clone, Debug)]
pub struct KnobParam {
    /// Name of the corresponding input parameter in the graph (must match exactly).
    pub param_name: String,
    /// Short label displayed below the knob (e.g., "Freq", "FM Dpth").
    pub label: String,
    /// How this knob interacts with its CV input port.
    pub input_mode: KnobInputMode,
}

impl KnobParam {
    /// Whether this parameter has a corresponding input port.
    pub fn has_input_port(&self) -> bool {
        !matches!(self.input_mode, KnobInputMode::KnobOnly)
    }

    /// Whether the knob should be disabled when connected.
    pub fn disable_when_connected(&self) -> bool {
        matches!(self.input_mode, KnobInputMode::Exposed)
    }
}

impl KnobParam {
    /// Create a new knob parameter with a specific input mode.
    pub fn with_mode(param_name: impl Into<String>, label: impl Into<String>, input_mode: KnobInputMode) -> Self {
        Self {
            param_name: param_name.into(),
            label: label.into(),
            input_mode,
        }
    }

    /// Create a new knob parameter (legacy compatibility).
    /// If `exposed_as_input` is true, creates an Exposed mode parameter.
    pub fn new(param_name: impl Into<String>, label: impl Into<String>, exposed_as_input: bool) -> Self {
        let mode = if exposed_as_input {
            KnobInputMode::Exposed
        } else {
            KnobInputMode::KnobOnly
        };
        Self::with_mode(param_name, label, mode)
    }

    /// Create a knob-only parameter (no corresponding input port).
    pub fn knob_only(param_name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::with_mode(param_name, label, KnobInputMode::KnobOnly)
    }

    /// Create an exposed parameter (has both knob and input port).
    /// When connected, the knob becomes read-only and displays the incoming signal.
    pub fn exposed(param_name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::with_mode(param_name, label, KnobInputMode::Exposed)
    }

    /// Create a modulatable parameter (has both knob and input port).
    /// When connected, the knob remains interactive for setting the base value,
    /// while CV modulates around that base. Ideal for filter cutoff, pitch, etc.
    pub fn modulatable(param_name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::with_mode(param_name, label, KnobInputMode::Modulatable)
    }
}

/// Where a knob is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnobPlace {
    /// On its own module.
    Module,
    /// On the face of a group it's pinned to, `depth` groups up from its
    /// module. `outer`: that group sits inside another.
    Face { depth: u8, outer: bool },
}

impl KnobPlace {
    fn is_face(self) -> bool {
        matches!(self, Self::Face { .. })
    }
}

/// Describes an LED indicator that appears in the node's bottom section.
///
/// LED indicators show the state of output ports (e.g., gate triggers, activity).
#[derive(Clone, Debug)]
pub struct LedIndicator {
    /// The output port index to monitor (0-based index among output ports).
    pub output_index: usize,
    /// Short label displayed below the LED.
    pub label: String,
    /// LED configuration (color, size, etc.).
    pub config: LedConfig,
}

impl LedIndicator {
    /// Create a new LED indicator for an output port.
    pub fn new(output_index: usize, label: impl Into<String>, config: LedConfig) -> Self {
        Self {
            output_index,
            label: label.into(),
            config,
        }
    }

    /// Create a green gate indicator (for trigger/gate outputs).
    pub fn gate(output_index: usize, label: impl Into<String>) -> Self {
        Self::new(output_index, label, LedConfig::green().with_size(10.0))
    }

    /// Create an orange activity indicator (for control signals).
    pub fn activity(output_index: usize, label: impl Into<String>) -> Self {
        Self::new(output_index, label, LedConfig::orange().with_size(10.0))
    }
}

/// Custom visualization a node draws above its knob row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeDisplay {
    /// No custom display.
    #[default]
    None,
    /// Oscillator: one cycle of the selected waveform.
    OscillatorWave,
    /// Noise: the white, pink and brown slopes, shimmering.
    NoiseSpectrum,
    /// LFO: waveform with a live phase marker.
    LfoWave,
    /// ADSR: envelope shape.
    Envelope,
    /// SVF Filter: frequency response curve.
    FilterResponse,
    /// Ladder Filter: frequency response curve.
    LadderResponse,
    /// Note In: a playable piano lit by the notes held.
    NotePiano,
    /// Quantizer: piano showing the scale and the note playing; keys toggle.
    ScalePiano,
    /// MIDI Monitor: scrolling event log.
    MidiLog,
    /// Oscilloscope: live traces.
    Scope,
    /// Step Sequencer: step grid.
    StepGrid,
    /// Chord Sequencer: the voicing on a keyboard, and a step per chord,
    /// named.
    ChordGrid,
    /// Audio Output: output stage level meter.
    OutputMeter,
    /// Mixer: a row per channel strip, with its jacks, its lane of the
    /// stereo field, meter, knobs, mute and solo, then the bus row. It
    /// places its own jacks and knobs.
    MixerStrips,
    /// Audio Input: the input's meters, and Follow scrolling past the
    /// Threshold.
    InputListen,
    /// Clock: four lamps for the beats of the bar, and where the time comes
    /// from.
    ClockBeat,
    /// Clock Divider: the count as a ring of beads, and the Gate's span.
    DividerRing,
    /// Logic: lamps for its inputs and outputs, and the CV against its
    /// Threshold.
    LogicLamps,
    /// Drum: the hit's envelope and pitch, lit as each hit plays.
    DrumHit,
    /// Resonator: the string drawn as it swings, or the body's modes as
    /// rings that fade as each dies away.
    ResonatorRings,
    /// Sampler: the recording's waveform, its markers, and a playhead for
    /// every voice playing it.
    SamplerWave,
    /// Chopper: the break's slices in their hues, and the row of steps
    /// under them, each tinted like the slice it plays.
    ChopperRow,
    /// Cloud: the source's waveform, and a spark for every grain reading it.
    CloudSparks,
    /// Trigger Sequencer: each lane's steps beside its Gate and Vel jacks,
    /// then the pattern tabs and the Chain.
    TriggerGrid,
    /// Looper: the loop as a ring with its playhead and overdub layers, and
    /// the footswitches.
    LooperRing,
    /// Slope: the rise and fall to proportion, with a dot riding them.
    SlopeShape,
    /// Dancer: a stick figure dancing on a stage, as big as its Size.
    Dancer,
    /// Link Out: a lamp per lane, what it sends, and whether Seoul hears.
    LinkLanes,
    /// MIDI Out: its device, a keyboard of the notes it holds, its CCs and clock.
    MidiOutKeys,
    /// Slicer: the row of steps, each drawn as the slice it cuts, and a
    /// dot riding the one playing.
    SlicerRow,
    /// Tape: two reels turning, and the tape trembling across a glowing head.
    TapeReels,
    /// Compressor: its transfer curve with a dot riding it, and the last
    /// two seconds of gain reduction hanging over the sound coming in.
    CompressorCurve,
    /// 3-Band EQ: the curve its bands add up to over the sound, with a
    /// dot per band to drag and wings on the mid band for its width.
    EqCurve,
    /// Stereo Delay: the train of echoes, each as loud as the feedback
    /// leaves it, lit as the sound coming in reaches it.
    DelayTaps,
    /// Chorus: each voice a strand swept by the LFO over the last two
    /// seconds, left and right taps braided, lit where sound passed.
    ChorusBraid,
    /// Distortion: its transfer curve, lit where the input reaches, and a
    /// sine at the input's level as the curve shapes it.
    DistortionCurve,
    /// Reverb: the room's decay, lows and highs, after the early
    /// reflections, with a dot riding down it as the room rings.
    ReverbTail,
    /// Vocoder: a bar per band, the voice's level there, on a frequency
    /// axis with the carrier's bands shifted by Formant.
    VocoderBands,
    /// Arranger: the song's sections beside its own jacks, each lane's
    /// level beside its CV and Gate, and a playhead through them all.
    Timeline,
    /// Scenes: a pad per scene, the From and To pads lit as Morph glides
    /// between them, and Morph riding a line from one to the other.
    ScenePads,
    /// Controls: a slim fader per slot, moving with the hardware, and what
    /// each hears.
    ControlFaders,
}

impl NodeDisplay {
    /// Whether the display lays out the node's knobs itself, in place of
    /// the knob rows under it.
    pub fn draws_own_knobs(self) -> bool {
        self == NodeDisplay::MixerStrips
    }
}

/// Data stored per node in the graph.
///
/// This contains information about which module type this node represents
/// and any per-instance display settings.
#[derive(Clone, Debug)]
pub struct SynthNodeData {
    /// The module type identifier (e.g., "osc.sine", "output.audio").
    pub module_id: &'static str,
    /// Display name shown in the node header.
    pub display_name: String,
    /// The category of this module (for header coloring).
    pub category: ModuleCategory,
    /// What the module does, shown as a tooltip on the header icon.
    pub description: &'static str,
    /// Custom visualization drawn above the knob row.
    pub display: NodeDisplay,
    /// Knob parameters to display in the bottom section of the node.
    pub knob_params: Vec<KnobParam>,
    /// Knobs per row before wrapping (0 keeps them all on one row).
    pub knobs_per_row: usize,
    /// LED indicators to display in the bottom section of the node.
    pub led_indicators: Vec<LedIndicator>,
    /// Output ports to monitor for feedback without LED indicators.
    /// Used for waveform displays, phase indicators, etc.
    pub monitored_outputs: Vec<usize>,
    /// Whether the module can be bypassed (a filter or effect with audio
    /// in and out). Bypassable nodes get a power switch in the header.
    pub bypassable: bool,
    /// Whether the module is bypassed: its audio input passes straight
    /// through, and the node is drawn dimmed.
    pub bypassed: bool,
    /// A module, or one of the nodes that make up a group.
    pub kind: NodeKind,
    /// The group this node sits inside, or `None` at the top of the patch.
    pub parent: Option<GroupId>,
    /// Knobs shown on the faces of the groups around the module: parameter
    /// name to how many groups up it shows (1 is the group it's in).
    pub pins: BTreeMap<String, u8>,
    /// What its pinned controls are called on the groups' faces, by
    /// parameter name, where they've been given a name of their own.
    pub face_labels: BTreeMap<String, String>,
    /// The file the module plays (a Sampler's), by sample key: an absolute
    /// path, or an example's (see [`crate::persistence::sample_files`]).
    pub file: Option<String>,
    /// Names given to things that aren't parameters (an Arranger's sections
    /// and lanes), by key.
    pub labels: BTreeMap<String, String>,
    /// Slice markers placed by hand (a Sampler's, sliced by Onsets), if
    /// they've been moved from where they were found.
    pub slice_markers: Option<crate::dsp::onsets::HandMarkers>,
    /// The scenes a Scenes module keeps, by slot letter.
    pub scenes: super::scenes::SceneBank,
}

/// How much of the node body shows through while it's bypassed.
const BYPASSED_OPACITY: f32 = 0.4;

/// What a knob cell draws in and answers to: its module, the graph around
/// it, the view's zoom, whether it's on the module or a group's face, and
/// where its edits go.
pub(super) struct KnobCellCtx<'a> {
    pub node_id: egui_node_graph2::NodeId,
    pub graph: &'a super::SynthGraph,
    pub user_state: &'a mut super::SynthGraphState,
    pub zoom: f32,
    pub place: KnobPlace,
    pub responses: &'a mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
}

impl<'a> KnobCellCtx<'a> {
    /// A knob on its own module.
    pub fn module(
        node_id: egui_node_graph2::NodeId,
        graph: &'a super::SynthGraph,
        user_state: &'a mut super::SynthGraphState,
        zoom: f32,
        responses: &'a mut Vec<NodeResponse<SynthResponse, SynthNodeData>>,
    ) -> Self {
        Self { node_id, graph, user_state, zoom, place: KnobPlace::Module, responses }
    }
}

/// A toggle's or dropdown's MIDI Learn: how to start it, what turns the
/// control now, and whether Learn is waiting for it.
pub(super) struct SwitchMidi {
    pub learn: SynthResponse,
    pub mapping: Option<super::MidiMappingInfo>,
    pub learning: bool,
}

impl SwitchMidi {
    /// Whether it shows the purple badge.
    pub fn badged(&self) -> bool {
        self.mapping.is_some() || self.learning
    }
}

/// A knob's MIDI mapping: what its badge shows and its menu offers.
#[derive(Default)]
struct KnobMidiConfig {
    /// Whether this knob has a MIDI CC, note or macro mapping.
    has_midi_mapping: bool,
    /// What turns it, if anything does.
    mapping: Option<super::MidiMappingInfo>,
    /// Whether this knob is the current MIDI Learn target.
    is_learn_target: bool,
    /// Parameter min value (for MIDI Learn).
    min_value: f32,
    /// Parameter max value (for MIDI Learn).
    max_value: f32,
}

impl SynthNodeData {
    /// Create new node data for a module.
    pub fn new(module_id: &'static str, display_name: impl Into<String>, category: ModuleCategory) -> Self {
        Self {
            module_id,
            display_name: display_name.into(),
            category,
            description: "",
            display: NodeDisplay::None,
            knob_params: Vec::new(),
            knobs_per_row: 0,
            led_indicators: Vec::new(),
            monitored_outputs: Vec::new(),
            bypassable: false,
            bypassed: false,
            kind: NodeKind::Module,
            parent: None,
            pins: BTreeMap::new(),
            face_labels: BTreeMap::new(),
            file: None,
            labels: BTreeMap::new(),
            slice_markers: None,
            scenes: Default::default(),
        }
    }

    /// Whether this node is a module, not part of a group.
    pub fn is_module(&self) -> bool {
        self.kind == NodeKind::Module
    }

    /// How many groups up a knob shows, or 0 if only on the module.
    pub fn pin_levels(&self, param_name: &str) -> u8 {
        self.pins.get(param_name).copied().unwrap_or(0)
    }

    /// The name a pinned control has been given for the groups' faces.
    pub fn face_label(&self, param_name: &str) -> Option<&str> {
        self.face_labels.get(param_name).map(String::as_str)
    }

    /// Builder method to set the tooltip description.
    pub fn with_description(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    /// Builder method to set the custom display.
    pub fn with_display(mut self, display: NodeDisplay) -> Self {
        self.display = display;
        self
    }

    /// Builder method to add knob parameters.
    pub fn with_knob_params(mut self, knob_params: Vec<KnobParam>) -> Self {
        self.knob_params = knob_params;
        self
    }

    /// Builder method to wrap the knob section into rows.
    pub fn with_knobs_per_row(mut self, knobs_per_row: usize) -> Self {
        self.knobs_per_row = knobs_per_row;
        self
    }

    /// Builder method to add LED indicators.
    pub fn with_led_indicators(mut self, led_indicators: Vec<LedIndicator>) -> Self {
        self.led_indicators = led_indicators;
        self
    }

    /// Builder method to add outputs that should be monitored for UI feedback.
    pub fn with_monitored_outputs(mut self, outputs: Vec<usize>) -> Self {
        self.monitored_outputs = outputs;
        self
    }

    /// Builder method to give the node a bypass switch.
    pub fn with_bypassable(mut self, bypassable: bool) -> Self {
        self.bypassable = bypassable;
        self
    }

    /// Get the header color for this node based on its category, or rose
    /// for a group.
    pub fn header_color(&self) -> Color32 {
        match self.kind {
            NodeKind::Module => self.category.color(),
            kind => super::group_face::header_color(kind),
        }
    }

    /// The header colour as drawn: the category colour, faded toward the
    /// node body's grey while bypassed, as if the module had powered down.
    pub fn titlebar_fill(&self) -> Color32 {
        let color = self.header_color();
        if !self.bypassed {
            return color;
        }
        let grey = 74.0;
        let mix = |c: u8| (c as f32 * 0.3 + grey * 0.7).round() as u8;
        Color32::from_rgb(mix(color.r()), mix(color.g()), mix(color.b()))
    }

    /// The ink the header is lettered in, which its title, icon, switch and
    /// close button share: a deep shade of the header's own colour, or near
    /// white, whichever stands out more. Bright headers get the dark ink, so
    /// "ADSR Envelope" reads on orange; deep ones like Output keep white.
    /// A bypassed module's ink sinks a little toward its greyed header.
    pub fn titlebar_ink(&self) -> Color32 {
        let fill = self.titlebar_fill();
        let deep = |c: u8| (c as f32 * 0.2).round() as u8;
        let dark = Color32::from_rgb(deep(fill.r()), deep(fill.g()), deep(fill.b()));
        let light = Color32::from_gray(250);
        let ink = if contrast_ratio(fill, dark) >= contrast_ratio(fill, light) { dark } else { light };
        if self.bypassed {
            ink.lerp_to_gamma(fill, 0.3)
        } else {
            ink
        }
    }

    /// Draws the bypass switch: the power symbol, lit while the module is in
    /// the signal path and dark when it's bypassed, like a pedal's LED.
    fn draw_power_switch(&self, painter: &egui::Painter, center: egui::Pos2, size: f32, hovered: bool) {
        let ink = self.titlebar_ink();
        let color = if self.bypassed && !hovered { ink.gamma_multiply(0.7) } else { ink };
        let radius = size * 0.4;
        let stroke = egui::Stroke::new((size * 0.13).max(1.0), color);

        if !self.bypassed {
            // A soft disc behind the symbol, so the switch reads as engaged
            painter.circle_filled(center, radius * 1.45, ink.gamma_multiply(0.12));
        }

        // The ring, open at the top where the stem passes through
        let gap = 0.75_f32;
        let start = -std::f32::consts::FRAC_PI_2 + gap;
        let sweep = std::f32::consts::TAU - 2.0 * gap;
        let points: Vec<egui::Pos2> = (0..=20)
            .map(|i| {
                let angle = start + sweep * i as f32 / 20.0;
                center + egui::vec2(angle.cos(), angle.sin()) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, stroke));
        painter.line_segment(
            [center + egui::vec2(0.0, -radius * 1.2), center + egui::vec2(0.0, -radius * 0.15)],
            stroke,
        );
    }

    /// Draw the category icon at the given position.
    /// Uses vector shapes for cross-platform reliability.
    fn draw_category_icon(&self, painter: &egui::Painter, center: egui::Pos2, size: f32, color: Color32) {
        let s = size * 0.5; // Half-size for calculations
        match self.category {
            ModuleCategory::Source => {
                // Sine wave icon
                let points: Vec<egui::Pos2> = (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0;
                        let x = center.x - s + t * s * 2.0;
                        let y = center.y - (t * std::f32::consts::TAU).sin() * s * 0.6;
                        egui::pos2(x, y)
                    })
                    .collect();
                painter.add(egui::Shape::line(points, egui::Stroke::new(1.5_f32, color)));
            }
            ModuleCategory::Filter => {
                // Triangle/slope icon (low-pass filter shape)
                let points = vec![
                    egui::pos2(center.x - s, center.y - s * 0.5),
                    egui::pos2(center.x, center.y - s * 0.5),
                    egui::pos2(center.x + s * 0.3, center.y + s * 0.5),
                    egui::pos2(center.x + s, center.y + s * 0.5),
                ];
                painter.add(egui::Shape::line(points, egui::Stroke::new(1.5_f32, color)));
            }
            ModuleCategory::Modulation => {
                // Diamond icon
                let points = vec![
                    egui::pos2(center.x, center.y - s * 0.7),
                    egui::pos2(center.x + s * 0.5, center.y),
                    egui::pos2(center.x, center.y + s * 0.7),
                    egui::pos2(center.x - s * 0.5, center.y),
                    egui::pos2(center.x, center.y - s * 0.7),
                ];
                painter.add(egui::Shape::line(points, egui::Stroke::new(1.5_f32, color)));
            }
            ModuleCategory::Effect => {
                // Star/sparkle icon
                for i in 0..4 {
                    let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                    let len = if i % 2 == 0 { s * 0.7 } else { s * 0.4 };
                    let dx = angle.cos() * len;
                    let dy = angle.sin() * len;
                    painter.line_segment(
                        [egui::pos2(center.x - dx, center.y - dy), egui::pos2(center.x + dx, center.y + dy)],
                        egui::Stroke::new(1.5_f32, color),
                    );
                }
            }
            ModuleCategory::Utility => {
                // Hash/grid icon
                let d = s * 0.4;
                painter.line_segment([egui::pos2(center.x - d, center.y - s * 0.6), egui::pos2(center.x - d, center.y + s * 0.6)], egui::Stroke::new(1.5_f32, color));
                painter.line_segment([egui::pos2(center.x + d, center.y - s * 0.6), egui::pos2(center.x + d, center.y + s * 0.6)], egui::Stroke::new(1.5_f32, color));
                painter.line_segment([egui::pos2(center.x - s * 0.6, center.y - d), egui::pos2(center.x + s * 0.6, center.y - d)], egui::Stroke::new(1.5_f32, color));
                painter.line_segment([egui::pos2(center.x - s * 0.6, center.y + d), egui::pos2(center.x + s * 0.6, center.y + d)], egui::Stroke::new(1.5_f32, color));
            }
            ModuleCategory::Output => {
                // Speaker cone icon
                painter.rect_stroke(
                    egui::Rect::from_center_size(egui::pos2(center.x - s * 0.3, center.y), egui::vec2(s * 0.4, s * 0.6)),
                    0.0,
                    egui::Stroke::new(1.5_f32, color),
                    egui::StrokeKind::Middle,
                );
                let cone = vec![
                    egui::pos2(center.x - s * 0.1, center.y - s * 0.3),
                    egui::pos2(center.x + s * 0.6, center.y - s * 0.6),
                    egui::pos2(center.x + s * 0.6, center.y + s * 0.6),
                    egui::pos2(center.x - s * 0.1, center.y + s * 0.3),
                ];
                painter.add(egui::Shape::line(cone, egui::Stroke::new(1.5_f32, color)));
            }
        }
    }

    /// A knob for a number parameter. It shows `shown` in place of its
    /// value while something else sets it, and a `locked` knob is greyed and
    /// can't be turned. Returns the knob's response, and the value the hand
    /// turned it to, if it did.
    fn number_knob(
        ui: &mut egui::Ui,
        value: f32,
        spec: &super::value_types::NumberSpec,
        shown: Option<f32>,
        locked: bool,
        config: &KnobConfig,
    ) -> (egui::Response, Option<f32>) {
        let mut display_val = shown.map(|shown| shown.clamp(spec.min, spec.max)).unwrap_or(value);
        let response = ui.add_enabled_ui(!locked, |ui| knob(ui, &mut display_val, config)).inner;
        let tolerance = (spec.max - spec.min).abs() * 1e-6;
        let turned = (!locked && (display_val - value).abs() > tolerance).then_some(display_val);
        (response, turned)
    }
}

impl SynthNodeData {
    /// One knob of the module's knob row, with its MIDI badge or signal dot
    /// and its right-click menu: on the module itself, or on the face of a
    /// group it's pinned to.
    pub(super) fn knob_cell(&self, ui: &mut egui::Ui, knob_param: &KnobParam, cx: &mut KnobCellCtx) {
        let (node_id, graph, zoom, place) = (cx.node_id, cx.graph, cx.zoom, cx.place);
        let user_state = &mut *cx.user_state;
        let responses = &mut *cx.responses;
        let engine_node_id = user_state.get_engine_node_id(node_id);
        let knob_size = 36.0 * zoom;
        // Find the corresponding input parameter by name
        if let Some(node) = graph.nodes.get(node_id) {
            if let Some((_name, input_id)) = node.inputs.iter().find(|(name, _)| *name == knob_param.param_name) {
                let input = graph.get_input(*input_id);
                let Some(current_param_index) = super::port_mapping::parameter_index(graph, node_id, *input_id) else {
                    return;
                };

                // Check if this param has an input port and if it's connected
                let is_connected = knob_param.has_input_port() && super::patched::input(graph, *input_id);

                // Something else in charge of it: a synced LFO's division,
                // a MIDI clock's tempo. Or reading what something else
                // makes of it: a Clock's tempo as its CV bends it
                let takeover = super::clock_display::knob_takeover(
                    self.module_id, &knob_param.param_name, node_id, graph, user_state,
                );

                // Should the knob be disabled? Only for Exposed mode, not for Modulatable
                let should_disable = takeover.as_ref().is_some_and(|takeover| takeover.locked) || (is_connected && knob_param.disable_when_connected());

                // Get signal feedback value from audio engine (if connected and available)
                let signal_value = if is_connected {
                    super::port_mapping::input_port_index(graph, node_id, *input_id)
                        .and_then(|port| user_state.get_input_value(engine_node_id?, port))
                } else {
                    None
                };

                // CV that reaches the knob through a jack of another name: a
                // VCA's CV moving its Level. The knob stays the hand's to turn
                let through_cv = super::knob_cv::jack(self.module_id, &knob_param.param_name)
                    .and_then(|jack| node.inputs.iter().find(|(name, _)| name == jack))
                    .filter(|(_, jack)| super::patched::input(graph, *jack))
                    .and_then(|(_, jack)| super::port_mapping::input_port_index(graph, node_id, *jack))
                    .and_then(|port| user_state.get_input_value(engine_node_id?, port));

                // The jack whose cable carries the knob's CV, if one's
                // plugged in: hovering either lights the other (#220)
                let fed_by = if is_connected {
                    Some(*input_id)
                } else {
                    super::knob_cv::jack(self.module_id, &knob_param.param_name)
                        .and_then(|jack| node.inputs.iter().find(|(name, _)| name == jack))
                        .map(|(_, jack)| *jack)
                        .filter(|jack| super::patched::input(graph, *jack))
                };
                let spotlight = fed_by.map_or(0.0, |jack| user_state.cable_spotlight.knob(jack));

                // Get MIDI mapping info for this parameter
                let midi_mapping = engine_node_id.and_then(|eid|
                    user_state.get_midi_mapping(eid, current_param_index));
                let is_learn_target = engine_node_id
                    .map(|eid| user_state.is_midi_learn_target(eid, current_param_index))
                    .unwrap_or(false);

                // Get min/max values for MIDI Learn
                let (min_value, max_value) = input.value.range();

                // Build MIDI config for the knob
                // A fader that hasn't picked the knob up yet shows where it is,
                // and once it has, its tick fades into the ring
                let pickup = engine_node_id.and_then(|eid| user_state.midi_pickups.get(&(eid, current_param_index))).map(|&(at, caught)| match caught {
                    None => (at, 1.0),
                    Some(when) => {
                        let now = ui.input(|i| i.time);
                        let fade = (1.0 - (now - when) / super::PICKUP_FADE).clamp(0.0, 1.0) as f32;
                        (super::scenes::travel(&input.value, input.value.actual_value()), fade)
                    }
                });
                let midi_config = KnobMidiConfig {
                    has_midi_mapping: midi_mapping.is_some(),
                    mapping: midi_mapping.cloned(),
                    is_learn_target,
                    min_value,
                    max_value,
                };

                // On a face, the name it's been given there
                let label = match place {
                    KnobPlace::Face { .. } => self.face_label(&knob_param.param_name).unwrap_or(&knob_param.label),
                    KnobPlace::Module => &knob_param.label,
                };

                // Render the knob based on value type, under an id of its
                // own so its right-click menu stays open as the node changes
                let knob_response = ui.push_id(("knob", node_id, current_param_index, place.is_face()), |ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(knob_size + 8.0 * zoom);

                        // Visual indicator for MIDI mapping or learn mode
                        let show_midi_indicator = midi_config.has_midi_mapping || midi_config.is_learn_target;
                        // Where the CV has carried a knob it modulates: the
                        // knob draws its way there, which says it's patched
                        // better than the dot does, so the dot gives way
                        let cv = match knob_param.input_mode {
                            KnobInputMode::Modulatable => signal_value,
                            _ => through_cv,
                        };
                        let modulated = match (&input.value, cv) {
                            (SynthValueType::Number { value, .. }, Some(cv)) if takeover.is_none() => {
                                super::knob_cv::effective(self.module_id, &knob_param.param_name, *value, cv, node_id, graph, user_state)
                            }
                            _ => None,
                        };
                        let show_connection_indicator = is_connected && !show_midi_indicator && modulated.is_none();

                        if show_midi_indicator {
                            // MIDI CC badge - purple for mapped, blinking for learn mode
                            let badge_color = if midi_config.is_learn_target {
                                // Blink effect for learn mode
                                let time = ui.ctx().input(|i| i.time);
                                let blink = ((time * 4.0).sin() > 0.0) as u8;
                                {
                                    let m = crate::app::theme::signal::MIDI;
                                    Color32::from_rgba_unmultiplied(m.r(), m.g(), m.b(), 128 + blink * 127)
                                }
                            } else {
                                crate::app::theme::signal::MIDI
                            };

                            // In the corner beside the knob's top right, which the
                            // dial never reaches, so a badge never moves the knob
                            let dot_size = 8.0 * zoom;
                            let badge_center = egui::pos2(
                                ui.cursor().left() + knob_size + 2.0 * zoom,
                                ui.cursor().top() + dot_size / 2.0,
                            );

                            // Draw badge background
                            ui.painter().circle_filled(badge_center, dot_size / 2.0 + 1.0 * zoom, badge_color);

                            // Draw "M" letter on badge
                            let text_pos = badge_center - egui::vec2(3.0 * zoom, 4.0 * zoom);
                            ui.painter().text(
                                text_pos,
                                egui::Align2::LEFT_TOP,
                                "M",
                                egui::FontId::proportional(8.0 * zoom),
                                Color32::WHITE,
                            );

                            // Request repaint for blinking effect
                            if midi_config.is_learn_target {
                                ui.ctx().request_repaint();
                            }
                        } else if show_connection_indicator {
                            // Orange color for Control signal (matches signal type color)
                            let indicator_color = if signal_value.is_some() {
                                crate::app::theme::signal::CONTROL
                            } else {
                                crate::app::theme::signal::CONTROL.gamma_multiply(0.45) // connected, no signal yet
                            };
                            // A small dot at the knob's top right, clear of the dial.
                            // It takes no room, so knobs in a row stay level
                            // whichever of them are patched
                            let dot_size = 6.0 * zoom;
                            let dot_center = egui::pos2(
                                ui.cursor().left() + knob_size - 1.0 * zoom,
                                ui.cursor().top() + dot_size / 2.0,
                            );
                            ui.painter().circle_filled(dot_center, dot_size / 2.0, indicator_color);
                        }

                        // Render knob based on the value type
                        // Note: We need to clone to render since we can't mutate through the graph reference
                        // The actual parameter change will be handled through the normal widget flow
                        // For modulatable params, pass None for signal_value so knob shows base value
                        let display_signal = if let Some(takeover) = &takeover {
                            takeover.value
                        } else if knob_param.disable_when_connected() {
                            signal_value
                        } else {
                            None // Modulatable: show base knob value, not CV signal
                        };
                        // Knob rows hold only continuous parameters (templates.rs)
                        let SynthValueType::Number { value, spec } = &input.value else {
                            return None;
                        };
                        // Range, default, unit and curve all come from the DSP definition
                        let config = KnobConfig {
                            size: knob_size,
                            range: spec.min..=spec.max,
                            default: spec.default,
                            format: spec.format(),
                            logarithmic: spec.logarithmic,
                            stepped: spec.stepped,
                            // An untitled knob sits under a column title instead
                            label: (!label.is_empty()).then(|| label.to_string()),
                            show_value: true,
                            value_text: takeover.map(|takeover| takeover.text),
                            accent: self.category.color(),
                            style: user_state.knob_style,
                            modulated: modulated.map(|m| m.clamp(spec.min, spec.max)),
                            spotlight,
                            pickup,
                            ..Default::default()
                        };
                        let (response, turned) = Self::number_knob(ui, *value, spec, display_signal, should_disable, &config);
                        if let Some(value) = turned {
                            responses.push(NodeResponse::User(SynthResponse::ParameterChanged {
                                node_id,
                                param_name: knob_param.param_name.clone(),
                                value,
                            }));
                        }
                        Some(response)
                    }).inner
                });

                // For a capture script to aim at: the dial, above its readout
                if let (Some(control), false) = (&knob_response.inner, place.is_face()) {
                    let r = control.rect;
                    let dial = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), r.width().min(r.height())));
                    super::marks::mark(ui.ctx(), node_id, format_args!("knob:{}", knob_param.param_name), dial);
                }

                // In Map Mode a click maps the knob instead of turning it (#426)
                let caught = engine_node_id.zip(knob_response.inner.as_ref()).and_then(|(eid, control)| {
                    let learn = SynthResponse::midi_learn_start(
                        eid,
                        current_param_index,
                        knob_param.param_name.clone(),
                        midi_config.min_value,
                        midi_config.max_value,
                        crate::persistence::MidiControl::Knob,
                    );
                    let dial = super::map_mode::Outline::Ring {
                        center: control.rect.min + egui::Vec2::splat(knob_size / 2.0),
                        radius: knob_size / 2.0 + 2.0 * zoom,
                    };
                    super::map_mode::catch(ui, control.rect, dial, &learn, user_state)
                });

                // The right-click menu hangs on the knob itself: an area laid
                // over it would be the topmost widget and take its clicks, so
                // a double-click would never reach the knob to reset it. A
                // disabled knob senses nothing, so it gets an area of its own.
                // In Map Mode, Map Mode's catch is over it, and carries it
                let interact_response = match (caught, knob_response.inner) {
                    (Some(caught), _) => {
                        responses.extend(caught.learn.map(NodeResponse::User));
                        caught.response
                    }
                    (None, Some(control)) if control.enabled() => control,
                    _ => ui.interact(
                        knob_response.response.rect,
                        egui::Id::unique(("knob_context", node_id, current_param_index, place.is_face())),
                        egui::Sense::click(),
                    ),
                };
                if let Some(jack) = fed_by.filter(|_| interact_response.hovered() || interact_response.dragged()) {
                    user_state.cable_spotlight.hover_knob(jack);
                }
                let hint = Hint::knob(self.module_id, &knob_param.param_name);
                let interact_response = hints::attach(interact_response, self.face_hint(hint, &knob_param.param_name, place));
                let named = if label.is_empty() { knob_param.param_name.as_str() } else { label };
                if place.is_face() && interact_response.secondary_clicked() {
                    super::group_face::reset_name_field(ui.ctx(), super::group_face::face_name_id(node_id, &knob_param.param_name));
                }

                // The right-click menu: the value, MIDI Learn and pinning
                if interact_response.secondary_clicked() {
                    super::knob_menu::reset_value_field(ui.ctx(), super::knob_menu::value_field_id(node_id, &knob_param.param_name));
                }
                let menu_response = crate::widgets::menu::context_menu(&interact_response, |ui| {
                    // The menu is as wide as its longest entry, not wrapped to the knob
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    if place.is_face() {
                        responses.extend(self.face_name_item(ui, node_id, &knob_param.param_name, named).map(NodeResponse::User));
                    }
                    // Its value, to type, and the way back to its default
                    if let SynthValueType::Number { value, spec } = &input.value {
                        let id = super::knob_menu::value_field_id(node_id, &knob_param.param_name);
                        if let Some(value) = super::knob_menu::value_items(ui, id, spec, *value, !should_disable, !place.is_face()) {
                            responses.push(NodeResponse::User(SynthResponse::ParameterChanged {
                                node_id,
                                param_name: knob_param.param_name.clone(),
                                value,
                            }));
                        }
                    }
                    // Learning a CC needs the module running in the engine
                    if let Some(engine_id) = engine_node_id {
                        ui.separator();
                        if is_learn_target {
                            // Already waiting for a CC: the way out
                            if ui.button("Cancel MIDI Learn").clicked() {
                                responses.push(NodeResponse::User(SynthResponse::MidiLearnCancel));
                                ui.close();
                            }
                        } else {
                            let learn = SynthResponse::midi_learn_start(
                                engine_id,
                                current_param_index,
                                knob_param.param_name.clone(),
                                midi_config.min_value,
                                midi_config.max_value,
                                crate::persistence::MidiControl::Knob,
                            );
                            responses.extend(midi_items(ui, midi_config.mapping.as_ref(), learn).map(NodeResponse::User));
                        }
                        // In a DAW, one of the plugin's eight macros can turn it
                        if user_state.hosted && !is_learn_target {
                            crate::widgets::menu::menu_button(ui, "Map to Macro", |ui| {
                                for slot in 0..crate::graph::MACRO_COUNT {
                                    let current = midi_config.mapping.as_ref().and_then(|m| m.macro_slot) == Some(slot);
                                    if ui.selectable_label(current, format!("Macro {}", slot + 1)).clicked() {
                                        responses.push(NodeResponse::User(SynthResponse::MapToMacro {
                                            engine_node_id: engine_id,
                                            param_index: current_param_index,
                                            param_name: knob_param.param_name.clone(),
                                            min_value: midi_config.min_value,
                                            max_value: midi_config.max_value,
                                            slot,
                                        }));
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("The DAW automates the macro, and the knob follows it");
                        }
                    }
                    if self.parent.is_some() || place.is_face() {
                        ui.separator();
                    }
                    responses.extend(self.pin_items(ui, node_id, &knob_param.param_name, place).map(NodeResponse::User));
                });
                // Set flag if context menu is open to prevent add-node menu
                if menu_response.is_some() {
                    user_state.widget_context_menu_open = true;
                }
            }
        }
    }

    /// The right-click menu items that pin a control to the faces of the
    /// groups around its module, or take it off: a knob's, a dropdown's or a
    /// toggle's. Nothing for a module that isn't in a group.
    pub(super) fn pin_items(
        &self,
        ui: &mut egui::Ui,
        node_id: egui_node_graph2::NodeId,
        param_name: &str,
        place: KnobPlace,
    ) -> Option<SynthResponse> {
        let pinned = self.pin_levels(param_name);
        let mut chosen = None;
        let mut pin = |ui: &mut egui::Ui, label: &str, hint: &str, levels: u8| {
            if ui.button(label).on_hover_text(hint).clicked() {
                chosen = Some(SynthResponse::PinKnob { node_id, param_name: param_name.to_string(), levels });
                ui.close();
            }
        };
        match place {
            KnobPlace::Module if self.parent.is_some() => {
                if pinned == 0 {
                    pin(ui, "Show on group", "Put this control on the face of the group it's in", 1);
                } else {
                    pin(ui, "Hide from group", "Take this control off the group's face", 0);
                }
            }
            KnobPlace::Face { depth, outer } => {
                pin(ui, "Hide from group", "Take this control off this group's face", depth - 1);
                if outer && pinned == depth {
                    pin(ui, "Show on outer group too", "Put this control on the face of the group around this one too", depth + 1);
                }
            }
            KnobPlace::Module => {}
        }
        chosen
    }

    /// The top of a face control's menu: the name it goes by on the face,
    /// `named`, selected to type over. Enter renames it; a blank name gives
    /// it back its module's own.
    fn face_name_item(&self, ui: &mut egui::Ui, node_id: egui_node_graph2::NodeId, param_name: &str, named: &str) -> Option<SynthResponse> {
        let id = super::group_face::face_name_id(node_id, param_name);
        let hint = "What this control is called on the group's face. Enter names it; a blank name goes back to the module's own";
        let name = super::group_face::name_field(ui, id, named, hint);
        ui.separator();
        name.map(|name| SynthResponse::NameFaceControl { node_id, param_name: param_name.to_string(), name })
    }

    /// A control's tooltip, which on a face where it's been named leads with
    /// that name and says what it drives.
    pub(super) fn face_hint(&self, hint: Option<Hint>, param_name: &str, place: KnobPlace) -> Option<Hint> {
        match (hint, place, self.face_label(param_name)) {
            (Some(hint), KnobPlace::Face { .. }, Some(name)) => Some(hint.named(name, &self.display_name)),
            (hint, ..) => hint,
        }
    }

    /// A dropdown's or toggle's right-click menu, among the module's inputs
    /// or on a group's face: its jack on the group, if it has a jack,
    /// pinning, and MIDI Learn (#423).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn switch_menu(
        &self,
        response: &egui::Response,
        node_id: egui_node_graph2::NodeId,
        param_name: &str,
        named: &str,
        value: &SynthValueType,
        place: KnobPlace,
        user_state: &mut super::SynthGraphState,
    ) -> Option<SynthResponse> {
        // MIDI Learn needs the module running in the engine
        let midi = self.midi_learn(node_id, param_name, value, user_state);
        if self.parent.is_none() && !place.is_face() && midi.is_none() {
            return None;
        }
        if place.is_face() && response.secondary_clicked() {
            super::group_face::reset_name_field(&response.ctx, super::group_face::face_name_id(node_id, param_name));
        }
        let jack_actions = match place {
            KnobPlace::Module => user_state.jack_actions(node_id, Side::Input, param_name).to_vec(),
            KnobPlace::Face { .. } => Vec::new(),
        };
        let mut chosen = None;
        let menu = crate::widgets::menu::context_menu(response, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            if place.is_face() {
                chosen = self.face_name_item(ui, node_id, param_name, named);
            }
            if !jack_actions.is_empty() {
                chosen = super::group_face::jack_items(ui, &jack_actions, node_id, Side::Input, param_name);
                ui.separator();
            }
            if self.parent.is_some() || place.is_face() {
                let pinned = self.pin_items(ui, node_id, param_name, place);
                chosen = chosen.take().or(pinned);
                ui.separator();
            }
            match &midi {
                Some(SwitchMidi { learning: true, .. }) => {
                    if ui.button("Cancel MIDI Learn").clicked() {
                        chosen = Some(SynthResponse::MidiLearnCancel);
                        ui.close();
                    }
                }
                Some(SwitchMidi { learn, mapping, .. }) => {
                    let learned = midi_items(ui, mapping.as_ref(), learn.clone());
                    chosen = chosen.take().or(learned);
                }
                None => {}
            }
        });
        if menu.is_some() {
            user_state.widget_context_menu_open = true;
        }
        chosen
    }

    /// What MIDI Learn offers a toggle or dropdown, when its module runs in
    /// the engine: a CC sets or sweeps it, a pad flips or steps it.
    pub(super) fn midi_learn(
        &self,
        node_id: egui_node_graph2::NodeId,
        param_name: &str,
        value: &SynthValueType,
        user_state: &super::SynthGraphState,
    ) -> Option<SwitchMidi> {
        let engine_id = user_state.get_engine_node_id(node_id)?;
        let index = super::SynthNodeTemplate::from_module_id(self.module_id)?.parameter_names().iter().position(|name| name == param_name)?;
        let control = match value {
            SynthValueType::Select { .. } => crate::persistence::MidiControl::Select,
            _ => crate::persistence::MidiControl::Toggle,
        };
        let (min, max) = value.range();
        Some(SwitchMidi {
            learn: SynthResponse::midi_learn_start(engine_id, index, param_name, min, max, control),
            mapping: user_state.get_midi_mapping(engine_id, index).cloned(),
            learning: user_state.is_midi_learn_target(engine_id, index),
        })
    }

    /// A module's right-click menu. Each action applies to the whole
    /// selection when the module is part of it.
    fn node_menu(
        &self,
        ui: &mut egui::Ui,
        node_id: egui_node_graph2::NodeId,
        graph: &super::SynthGraph,
        responses: &mut Vec<NodeResponse<SynthResponse, Self>>,
    ) {
        ui.set_min_width(170.0);
        let mut item = |ui: &mut egui::Ui, label: &str, shortcut: &str, response: SynthResponse| {
            if ui.add(egui::Button::new(label).shortcut_text(crate::app::shortcuts::keys(ui.ctx(), shortcut))).clicked() {
                responses.push(NodeResponse::User(response));
                ui.close();
            }
        };
        item(ui, "Duplicate", "Ctrl+D", SynthResponse::DuplicateNode(node_id));
        item(ui, "Copy", "Ctrl+C", SynthResponse::CopyNode(node_id));
        if self.bypassable {
            let label = if self.bypassed { "Switch on" } else { "Bypass" };
            item(ui, label, "Ctrl+B", SynthResponse::ToggleBypass(node_id));
        }
        item(ui, "Reset to defaults", "", SynthResponse::ResetNode(node_id));
        item(ui, "Group", "Ctrl+G", SynthResponse::GroupNode(node_id));
        let mut recorded = None;
        if !super::resample::sources(graph, node_id).is_empty() {
            ui.separator();
            recorded = super::resample::module_items(ui, graph, node_id);
        }
        ui.separator();
        item(ui, "Delete", "Del", SynthResponse::DeleteNode(node_id));
        responses.extend(recorded.map(NodeResponse::User));
    }
}

/// How far apart two colours sit in lightness, as WCAG measures legibility:
/// 1 for the same colour, 21 for black on white, 4.5 or more for body text
fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let luminance = |c: Color32| {
        let linear = egui::Rgba::from(c);
        0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// An output's label, held back until the node's width is known so it can
/// sit against its port on the right edge rather than at the left
#[derive(Clone)]
struct DeferredOutputLabel {
    name: String,
    slot: egui::layers::ShapeIdx,
    rect: egui::Rect,
    shapes: Vec<egui::Shape>,
}

/// The faint rule in the module's category colour between its ports and
/// the face drawn under them, with room above and below.
fn category_rule(ui: &mut egui::Ui, category_color: Color32, zoom: f32) {
    ui.add_space(4.0 * zoom);
    let separator_color = Color32::from_rgba_unmultiplied(
        category_color.r(),
        category_color.g(),
        category_color.b(),
        64,
    );
    let margin = 4.0 * zoom;
    let rect = ui.available_rect_before_wrap();
    ui.painter().hline(
        (rect.left() + margin)..=(rect.right() - margin),
        ui.cursor().top(),
        egui::Stroke::new(1.0 * zoom, separator_color),
    );
    ui.add_space(4.0 * zoom);
}

fn deferred_labels_id(node_id: egui_node_graph2::NodeId) -> egui::Id {
    egui::Id::unique((node_id, "deferred_output_labels"))
}

/// Reserves the label's place in the paint order now, and its shapes for
/// [`place_output_labels`] to move once the node's width is settled. The
/// label is laid out at its natural width, so it never widens the node.
pub(super) fn defer_output_label(
    ui: &mut egui::Ui,
    node_id: egui_node_graph2::NodeId,
    name: &str,
    rect: egui::Rect,
    shapes: Vec<egui::Shape>,
) {
    let label = DeferredOutputLabel {
        name: name.to_owned(),
        slot: ui.painter().add(egui::Shape::Noop),
        rect,
        shapes,
    };
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<DeferredOutputLabel>>(deferred_labels_id(node_id))
            .push(label);
    });
}

/// An output's right-click menu, over its label at `rect`: shown on the
/// group around it, or not, or the jack it is taken away; and an audio
/// output resampled into a new Sampler.
fn output_menu(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    node_id: egui_node_graph2::NodeId,
    graph: &super::SynthGraph,
    param_name: &str,
    user_state: &mut super::SynthGraphState,
) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let source = super::resample::source_of(graph, node_id, param_name);
    if user_state.jack_actions(node_id, Side::Output, param_name).is_empty() && source.is_none() {
        return Vec::new();
    }
    let response = ui.interact(rect, egui::Id::unique((node_id, param_name, "output menu")), egui::Sense::click());
    let more = |ui: &mut egui::Ui| source.as_ref().and_then(|source| super::resample::port_item(ui, graph, node_id, source));
    let more: Option<&dyn Fn(&mut egui::Ui) -> Option<SynthResponse>> = if source.is_some() { Some(&more) } else { None };
    super::group_face::port_menu_with(&response, node_id, Side::Output, param_name, user_state, more)
        .map(NodeResponse::User)
        .into_iter()
        .collect()
}

/// Slides each deferred output label flush with the node's right edge,
/// beside its port, and gives it its hover hint and right-click menu there
fn place_output_labels(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    node_id: egui_node_graph2::NodeId,
    module_id: &str,
    graph: &super::SynthGraph,
    user_state: &mut super::SynthGraphState,
) -> Vec<NodeResponse<SynthResponse, SynthNodeData>> {
    let mut responses = Vec::new();
    let labels = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<Vec<DeferredOutputLabel>>(deferred_labels_id(node_id)))
        .unwrap_or_default();
    let right = ui.min_rect().right();
    for label in labels {
        let offset = egui::vec2((right - label.rect.right()).max(0.0), 0.0);
        let shapes = label
            .shapes
            .into_iter()
            .map(|mut shape| {
                shape.translate(offset);
                shape
            })
            .collect::<Vec<_>>();
        painter.set(label.slot, egui::Shape::Vec(shapes));

        let rect = label.rect.translate(offset);
        responses.extend(output_menu(ui, rect, node_id, graph, &label.name, user_state));
        let id = ui.scope_id().with((node_id, "output_label", &label.name));
        let response = ui.interact(rect, id, egui::Sense::hover());
        hints::attach(response, Hint::output(module_id, &label.name));
    }
    responses
}

impl NodeDataTrait for SynthNodeData {
    type Response = SynthResponse;
    type UserState = super::SynthGraphState;
    type DataType = super::SynthDataType;
    type ValueType = super::SynthValueType;

    fn top_bar_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: egui_node_graph2::NodeId,
        graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        user_state: &mut Self::UserState,
        zoom: f32,
    ) -> Vec<NodeResponse<Self::Response, Self>>
    where
        Self::Response: UserResponseTrait,
    {
        if !self.is_module() {
            return super::group_face::top_bar(self, ui, node_id, user_state, zoom);
        }
        let mut responses = Vec::new();

        // Right-clicking the module opens its menu. The editor senses clicks
        // on the whole node under this id, before drawing what's inside it
        if let Some(window) = ui.ctx().read_response(egui::Id::unique((node_id, "window"))) {
            if window.secondary_clicked() {
                responses.push(NodeResponse::User(SynthResponse::NodeSelected(node_id)));
            }
            let menu = crate::widgets::menu::context_menu(&window, |ui| self.node_menu(ui, node_id, graph, &mut responses));
            if menu.is_some() {
                user_state.widget_context_menu_open = true;
            }
        }

        // The bypass switch leads the header, like a footswitch
        if self.bypassable {
            let switch_size = 13.0 * zoom;
            let (switch_rect, switch) = ui.allocate_exact_size(
                egui::vec2(switch_size + 3.0 * zoom, switch_size),
                egui::Sense::click(),
            );
            let center = egui::pos2(switch_rect.left() + switch_size * 0.5, switch_rect.center().y);
            self.draw_power_switch(ui.painter(), center, switch_size, switch.hovered());
            super::marks::mark(ui.ctx(), node_id, "bypass", egui::Rect::from_center_size(center, egui::Vec2::splat(switch_size)));
            let hint = if self.bypassed {
                "Bypassed: audio passes straight through. Click to switch back in (Ctrl+B)"
            } else {
                "Bypass: pass audio straight through (Ctrl+B)"
            };
            if switch.on_hover_text(crate::app::shortcuts::keys(ui.ctx(), hint)).clicked() {
                responses.push(NodeResponse::User(SynthResponse::ToggleBypass(node_id)));
            }
        }

        // Allocate space for the category icon (drawn before the title)
        let icon_size = 14.0 * zoom;
        let icon_padding = 4.0 * zoom;
        let (icon_rect, response) = ui.allocate_exact_size(
            egui::vec2(icon_size + icon_padding, icon_size),
            egui::Sense::hover(),
        );
        if !self.description.is_empty() {
            response.on_hover_text(self.description);
        }

        // Draw the category icon centered in the allocated space
        let icon_center = egui::pos2(
            icon_rect.left() + icon_size * 0.5,
            icon_rect.center().y,
        );
        self.draw_category_icon(ui.painter(), icon_center, icon_size, self.titlebar_ink());

        // The Arranger shows its name and the section playing, after the title
        if self.display == NodeDisplay::Timeline {
            if let Some(note) = super::arranger_display::title_note(graph, user_state, node_id) {
                // After the title, which the editor draws next
                let title_font = self.titlebar_text_style().resolve(ui.style());
                let title = ui.painter().layout_no_wrap(graph[node_id].label.clone(), title_font, Color32::WHITE).size().x;
                let font = egui::FontId::proportional(11.0 * zoom);
                let ink = self.titlebar_ink();
                let galley = ui.painter().layout_no_wrap(note, font, ink.gamma_multiply(0.85));
                let at = egui::pos2(icon_rect.right() + title + 12.0 * zoom, icon_rect.center().y - galley.size().y / 2.0);
                ui.painter().galley(at, galley, ink);
            }
        }

        responses
    }

    fn bottom_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: egui_node_graph2::NodeId,
        graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        user_state: &mut Self::UserState,
        zoom: f32,
    ) -> Vec<NodeResponse<Self::Response, Self>>
    where
        Self::Response: UserResponseTrait,
    {
        // Output labels keep the full opacity they had before the fade below
        let label_painter = ui.painter().clone();
        if !self.is_module() {
            let mut responses = super::group_face::body(self, ui, node_id, graph, user_state, zoom);
            responses.extend(place_output_labels(ui, &label_painter, node_id, self.module_id, graph, user_state));
            return responses;
        }
        let mut responses = Vec::new();

        // A bypassed module's controls fade back but stay adjustable
        if self.bypassed {
            ui.multiply_opacity(BYPASSED_OPACITY);
        }

        // Get the engine node ID for looking up input values
        let engine_node_id = user_state.get_engine_node_id(node_id);

        // Each module's own face, above its knob row. The match has no
        // catch-all, so a new NodeDisplay doesn't compile until it's drawn
        let color = self.category.color();
        match self.display {
            NodeDisplay::None => {}
            NodeDisplay::MidiLog => {
                category_rule(ui, color, zoom);
                super::midi_log_display::midi_log_display(ui, node_id, graph, user_state, zoom);
            }
            // Audio Output: the output stage's level meter
            NodeDisplay::OutputMeter => {
                category_rule(ui, color, zoom);
                ui.add_space(2.0 * zoom);
                super::output_meter_display::output_meter_display(ui, node_id, graph, user_state, zoom);
            }
            NodeDisplay::Scope => {
                category_rule(ui, color, zoom);
                super::scope_display::scope_display(ui, node_id, graph, user_state, zoom);
            }
            NodeDisplay::StepGrid => {
                category_rule(ui, color, zoom);
                step_sequencer_display(ui, zoom, node_id, graph, user_state, &mut responses);
            }
            NodeDisplay::ChordGrid => {
                ui.add_space(4.0 * zoom);
                let edits = super::chord_grid::chord_grid(ui, node_id, graph, user_state, zoom);
                responses.extend(edits.into_iter().map(NodeResponse::User));
            }
            // Oscillator: the wave it plays, and its unison voices
            NodeDisplay::OscillatorWave => super::oscillator_display::oscillator_display(ui, node_id, graph, user_state, zoom),
            // Noise: the three slopes, with the patched ones lit
            NodeDisplay::NoiseSpectrum => {
                category_rule(ui, color, zoom);
                super::noise_display::noise_display(ui, node_id, graph, user_state, zoom);
            }
            // ADSR: the envelope's shape, and where a note is in it
            NodeDisplay::Envelope => super::envelope_display::envelope_display(ui, node_id, graph, user_state, zoom),
            // Filters: what each output passes, at the cutoff CV has taken it to
            NodeDisplay::FilterResponse => {
                super::filter_display::filter_display(ui, super::filter_display::Filter::Svf, node_id, graph, user_state, zoom);
            }
            NodeDisplay::LadderResponse => {
                super::filter_display::filter_display(ui, super::filter_display::Filter::Ladder, node_id, graph, user_state, zoom);
            }
            // LFO: its cycle, with a dot where it is
            NodeDisplay::LfoWave => super::lfo_display::lfo_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::NotePiano => {
                category_rule(ui, color, zoom);
                super::piano_display::note_piano_display(ui, node_id, graph, user_state, zoom);
            }
            // Quantizer: its scale on a one-octave piano, the notes it plays
            // glowing. Clicking a key adds it to or takes it out of the scale
            NodeDisplay::ScalePiano => {
                category_rule(ui, color, zoom);
                let edits = super::scale_display::scale_display(ui, node_id, graph, user_state, zoom);
                responses.extend(edits.into_iter().map(NodeResponse::User));
            }
            // Clock: the beat in the bar, and where the time comes from
            NodeDisplay::ClockBeat => super::clock_display::clock_display(ui, node_id, graph, user_state, zoom),
            // Mixer: a row per channel strip, its jacks, lane, meter, knobs and
            // buttons, and the bus row below
            NodeDisplay::MixerStrips => {
                responses.extend(super::mixer_strips::mixer_strips(self, ui, node_id, graph, user_state, zoom));
            }
            NodeDisplay::InputListen => {
                if let Some((param_name, value)) = super::input_display::input_display(ui, node_id, graph, user_state, zoom) {
                    responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value }));
                }
            }
            NodeDisplay::DividerRing => super::divider_display::divider_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::Dancer => {
                if let Some((param_name, value)) = super::dancer_display::dancer_display(ui, node_id, graph, user_state, zoom) {
                    responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value }));
                }
            }
            NodeDisplay::SlopeShape => super::slope_display::slope_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::LinkLanes => super::link_display::link_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::MidiOutKeys => super::midi_out_display::midi_out_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::TapeReels => super::tape_display::tape_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::VocoderBands => super::vocoder_display::vocoder_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::CompressorCurve => super::compressor_display::compressor_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::ReverbTail => super::reverb_display::reverb_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::DistortionCurve => super::distortion_display::distortion_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::ChorusBraid => super::chorus_display::chorus_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::DelayTaps => super::delay_display::delay_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::EqCurve => {
                let changes = super::eq_display::eq_display(ui, node_id, graph, user_state, zoom);
                responses.extend(changes.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::ControlFaders => {
                let asked = super::controls_face::controls_face(ui, node_id, graph, user_state, zoom);
                responses.extend(asked.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::ScenePads => {
                if let Some(response) = super::scenes_display::scenes_display(ui, node_id, graph, user_state, zoom) {
                    responses.push(NodeResponse::User(response));
                }
            }
            NodeDisplay::ResonatorRings => super::resonator_display::resonator_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::DrumHit => super::drum_display::drum_display(ui, node_id, graph, user_state, zoom),
            NodeDisplay::SamplerWave => {
                let actions = super::sampler_display::sampler_display(ui, node_id, graph, user_state, zoom);
                responses.extend(actions.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::ChopperRow => {
                let actions = super::chopper_display::chopper_display(ui, node_id, graph, user_state, zoom);
                responses.extend(actions.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::CloudSparks => {
                let actions = super::cloud_display::cloud_display(ui, node_id, graph, user_state, zoom);
                responses.extend(actions.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::SlicerRow => {
                let edits = super::slicer_display::slicer_display(ui, node_id, graph, user_state, zoom);
                responses.extend(edits.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::LooperRing => {
                let actions = super::looper_display::looper_display(ui, node_id, graph, user_state, zoom);
                responses.extend(actions.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::TriggerGrid => {
                let edits = super::trigger_display::pattern_bar(ui, node_id, graph, user_state, zoom);
                responses.extend(edits.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::Timeline => {
                let edits = super::arranger_display::section_bar(ui, node_id, graph, user_state, zoom);
                responses.extend(edits.into_iter().map(NodeResponse::User));
            }
            NodeDisplay::LogicLamps => {
                if let Some((param_name, value)) = super::logic_display::logic_display(ui, node_id, graph, user_state, zoom) {
                    responses.push(NodeResponse::User(SynthResponse::ParameterChanged { node_id, param_name, value }));
                }
            }
        }

        // Render horizontal row of knobs if this node has knob parameters
        if !self.knob_params.is_empty() && !self.display.draws_own_knobs() {
            // Add spacing before the knob row (separator removed - it was expanding to fill available width)
            ui.add_space(8.0 * zoom);

            // Render knobs - just use a simple horizontal layout
            // Centering would require knowing the final node width which we don't have yet
            let per_row = match self.knobs_per_row {
                0 => self.knob_params.len(),
                n => n,
            };
            for row in self.knob_params.chunks(per_row) {
            ui.horizontal(|ui| {
                for knob_param in row {
                    self.knob_cell(ui, knob_param, &mut KnobCellCtx::module(node_id, graph, user_state, zoom, &mut responses));
                }
                });
            }
        }

        // Render LED indicators if this node has any
        if !self.led_indicators.is_empty() {
            // Add spacing before LEDs
            ui.add_space(4.0 * zoom);

            // Render LEDs in a horizontal layout
            ui.horizontal(|ui| {
                for led_indicator in &self.led_indicators {
                    // Get the output value from the audio engine
                    let brightness = engine_node_id
                        .and_then(|eid| user_state.get_output_value(eid, led_indicator.output_index))
                        .unwrap_or(0.0);

                    // Render the LED with label
                    ui.vertical(|ui| {
                        ui.set_min_width(20.0 * zoom);
                        let config = led_indicator.config.clone()
                            .with_label(&led_indicator.label)
                            .with_size(led_indicator.config.size * zoom);
                        led(ui, brightness, &config);
                    });
                }
            });
        }

        // Everything that sets the node's width has been laid out now
        responses.extend(place_output_labels(ui, &label_painter, node_id, self.module_id, graph, user_state));

        responses
    }

    fn output_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: egui_node_graph2::NodeId,
        graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        user_state: &mut Self::UserState,
        param_name: &str,
    ) -> Vec<NodeResponse<Self::Response, Self>>
    where
        Self::Response: UserResponseTrait,
    {
        // The Trigger Sequencer draws each lane of its grid beside the lane's jacks
        if self.display == NodeDisplay::TriggerGrid {
            if let Some(edits) = super::trigger_display::output_row(ui, node_id, graph, user_state, param_name) {
                return edits.into_iter().map(NodeResponse::User).collect();
            }
        }
        // The Arranger draws its timeline the same way, a lane beside its jacks
        if self.display == NodeDisplay::Timeline {
            if let Some(edits) = super::arranger_display::output_row(ui, node_id, graph, user_state, param_name) {
                return edits.into_iter().map(NodeResponse::User).collect();
            }
        }

        // Calculate the text width and allocate exactly that much space
        // This allows the node to be narrow while still showing the label
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let text_color = ui.visuals().widgets.noninteractive.fg_stroke.color;
        let galley = ui.painter().layout_no_wrap(param_name.to_string(), font_id.clone(), text_color);
        // As big as the text would be if fonts scaled smoothly (#255)
        let text_size = egui::vec2(
            super::steady_text::text_width(ui, param_name, &font_id),
            super::steady_text::row_height(ui, &font_id),
        );
        let (drawn_width, drawn_height) = (galley.size().x, galley.size().y);
        let text_top = |rect: egui::Rect| rect.center().y - drawn_height / 2.0;

        // A polyphonic output also shows how many channels it carries
        let outputs = &graph[node_id].outputs;
        let poly = outputs.iter().position(|(name, _)| name == param_name).and_then(|index| {
            let channels = user_state.output_channel_count(node_id, index);
            (channels > 1).then(|| (channels, graph.get_output(outputs[index].1).typ.0.color()))
        });
        let Some((channels, color)) = poly else {
            // Allocate exactly the text size, not the full available width
            let (_, rect) = ui.allocate_space(text_size);
            // Flush right, against the jack, whatever the text gains or loses
            let shapes = vec![egui::Shape::galley(egui::pos2(rect.max.x - drawn_width, text_top(rect)), galley, text_color)];
            defer_output_label(ui, node_id, param_name, rect, shapes);
            return Vec::new();
        };

        // The count sits in a pill of the cable's color, between the label
        // and the port, sized from the label so it follows the zoom
        let badge_font = egui::FontId::monospace(font_id.size * 0.72);
        let badge_text = color.lerp_to_gamma(Color32::WHITE, 0.35);
        let badge_galley = ui.painter().layout_no_wrap(format!("×{channels}"), badge_font.clone(), badge_text);
        let pad = egui::vec2(font_id.size * 0.3, font_id.size * 0.08);
        let badge_size = egui::vec2(
            super::steady_text::text_width(ui, &format!("×{channels}"), &badge_font),
            super::steady_text::row_height(ui, &badge_font),
        ) + 2.0 * pad;
        let gap = font_id.size * 0.3;

        let size = egui::vec2(text_size.x + gap + badge_size.x, text_size.y.max(badge_size.y));
        let (_, rect) = ui.allocate_space(size);
        let badge = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - badge_size.x, rect.center().y - badge_size.y / 2.0),
            badge_size,
        );
        let shapes = vec![
            egui::Shape::galley(egui::pos2(rect.min.x, text_top(rect)), galley, text_color),
            egui::Shape::rect_filled(badge, badge_size.y / 2.0, color.gamma_multiply(0.22)),
            egui::Shape::rect_stroke(badge, badge_size.y / 2.0, egui::Stroke::new(1.0_f32, color.gamma_multiply(0.7)), egui::StrokeKind::Middle),
            egui::Shape::galley(egui::pos2(badge.min.x + pad.x, badge.center().y - badge_galley.size().y / 2.0), badge_galley, badge_text),
        ];
        defer_output_label(ui, node_id, param_name, rect, shapes);
        Vec::new()
    }

    fn titlebar_color(
        &self,
        ui: &egui::Ui,
        node_id: egui_node_graph2::NodeId,
        _graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        user_state: &mut Self::UserState,
    ) -> Option<Color32> {
        // The category-based header color, flushed red just after the
        // module made a NaN and was reset
        let glow = user_state.get_engine_node_id(node_id).map_or(0.0, |id| user_state.faults.glow(id));
        if glow > 0.0 {
            ui.ctx().request_repaint();
        }
        Some(super::fault_flash::tint(self.titlebar_fill(), glow))
    }

    fn titlebar_text_color(
        &self,
        _ui: &egui::Ui,
        _node_id: egui_node_graph2::NodeId,
        _graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        _user_state: &mut Self::UserState,
    ) -> Option<Color32> {
        Some(self.titlebar_ink())
    }

    fn titlebar_text_style(&self) -> egui::TextStyle {
        crate::app::theme::title_text_style()
    }

    /// The Mixer lays its jacks out beside its channel rows.
    fn places_port(
        &self,
        node_id: egui_node_graph2::NodeId,
        graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        param: egui_node_graph2::AnyParameterId,
    ) -> bool {
        if self.display != NodeDisplay::MixerStrips {
            return false;
        }
        let Some(node) = graph.nodes.get(node_id) else {
            return false;
        };
        let name = match param {
            egui_node_graph2::AnyParameterId::Input(id) => node.inputs.iter().find(|(_, i)| *i == id).map(|(n, _)| n),
            egui_node_graph2::AnyParameterId::Output(id) => node.outputs.iter().find(|(_, o)| *o == id).map(|(n, _)| n),
        };
        name.is_some_and(|name| super::mixer_strips::places(name))
    }

    /// A group's Inputs and Outputs only go with the group.
    fn can_delete(
        &self,
        _node_id: egui_node_graph2::NodeId,
        _graph: &egui_node_graph2::Graph<Self, Self::DataType, Self::ValueType>,
        _user_state: &mut Self::UserState,
    ) -> bool {
        !self.kind.is_proxy()
    }
}

/// How a CC moves the knob or dropdown it's mapped to (#424): a fader or
/// an endless encoder, picking the knob up or jumping it, and which way up.
fn style_items(ui: &mut egui::Ui, mapping: &super::MidiMappingInfo, learn: &SynthResponse) -> Option<SynthResponse> {
    use crate::persistence::{MidiControl, MidiEncoding, MidiSource};
    let SynthResponse::MidiLearnStart { engine_node_id, param_index, scene, .. } = *learn else { return None };
    if mapping.source != MidiSource::Cc || mapping.macro_slot.is_some() || !matches!(mapping.control, MidiControl::Knob | MidiControl::Select) {
        return None;
    }
    let mut style = mapping.style();
    crate::widgets::menu::menu_button(ui, "Hardware", |ui| {
        let kinds = [
            (MidiEncoding::Absolute, "Knob or fader", "It has ends, and sends where it is"),
            (MidiEncoding::Relative64, "Encoder (64 ± steps)", "Endless: sends 65 for a step up, 63 for a step down"),
            (MidiEncoding::TwosComplement, "Encoder (1 / 127)", "Endless: sends 1 for a step up, 127 for a step down"),
        ];
        for (encoding, name, tip) in kinds {
            if ui.selectable_label(style.encoding == encoding, name).on_hover_text(tip).clicked() {
                style.encoding = encoding;
            }
        }
    });
    if mapping.control == MidiControl::Knob && style.encoding == MidiEncoding::Absolute {
        let mut pick_up = !style.jump;
        let tip = "The knob waits where it is until the hardware reaches it, rather than jumping to the hardware";
        ui.checkbox(&mut pick_up, "Pick up").on_hover_text(tip);
        style.jump = !pick_up;
    }
    ui.checkbox(&mut style.invert, "Invert").on_hover_text("The hardware's top is the bottom here");
    (style != mapping.style()).then_some(SynthResponse::MidiStyle { engine_node_id, param_index, scene, style })
}

/// A mapped control's MIDI items, after any value of its own: what turns it,
/// Clear and Re-learn, or Learn when nothing does yet. `learn` is the
/// response that starts MIDI Learn on it.
pub(super) fn midi_items(ui: &mut egui::Ui, mapping: Option<&super::MidiMappingInfo>, learn: SynthResponse) -> Option<SynthResponse> {
    let tip = "Move a knob or fader, or hit a pad or key, on your MIDI controller";
    let Some(mapping) = mapping else {
        let clicked = ui.button("Learn MIDI").on_hover_text(tip).clicked();
        if clicked {
            ui.close();
        }
        return clicked.then_some(learn);
    };
    ui.label(RichText::new(mapping.text()).small().weak());
    let mut chosen = style_items(ui, mapping, &learn);
    ui.separator();
    if ui.button("Clear MIDI").clicked() {
        if let SynthResponse::MidiLearnStart { engine_node_id, param_index, scene, .. } = &learn {
            chosen = Some(SynthResponse::MidiLearnClear { engine_node_id: *engine_node_id, param_index: *param_index, scene: *scene });
        }
        ui.close();
    }
    if ui.button("Re-learn MIDI").on_hover_text(tip).clicked() {
        chosen = Some(learn);
        ui.close();
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synth_node_data_creation() {
        let data = SynthNodeData::new(
            "osc.sine",
            "Sine Oscillator",
            ModuleCategory::Source,
        );

        assert_eq!(data.module_id, "osc.sine");
        assert_eq!(data.display_name, "Sine Oscillator");
        assert_eq!(data.category, ModuleCategory::Source);
    }

    #[test]
    fn test_header_color() {
        let source = SynthNodeData::new("test", "Test", ModuleCategory::Source);
        let filter = SynthNodeData::new("test", "Test", ModuleCategory::Filter);
        let output = SynthNodeData::new("test", "Test", ModuleCategory::Output);

        // Colors should match the category colors
        assert_eq!(source.header_color(), ModuleCategory::Source.color());
        assert_eq!(filter.header_color(), ModuleCategory::Filter.color());
        assert_eq!(output.header_color(), ModuleCategory::Output.color());
    }

    #[test]
    fn titles_are_legible_on_every_header() {
        let categories = [
            ModuleCategory::Source,
            ModuleCategory::Filter,
            ModuleCategory::Modulation,
            ModuleCategory::Effect,
            ModuleCategory::Utility,
            ModuleCategory::Output,
        ];
        for category in categories {
            let mut node = SynthNodeData::new("test", "Test", category);
            // Live headers hold WCAG's 4.5:1 for text
            let contrast = contrast_ratio(node.titlebar_fill(), node.titlebar_ink());
            assert!(contrast >= 4.5, "{:?} title contrast {:.1}", category, contrast);

            // A bypassed one is quieter on purpose, but still readable
            node.bypassed = true;
            let contrast = contrast_ratio(node.titlebar_fill(), node.titlebar_ink());
            assert!(contrast >= 3.0, "{:?} bypassed title contrast {:.1}", category, contrast);
        }
    }

    #[test]
    fn contrast_ratio_spans_one_to_twenty_one() {
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.1);
    }

    #[test]
    fn test_category_icons() {
        // Verify each category has a node data struct
        let categories = [
            ModuleCategory::Source,
            ModuleCategory::Filter,
            ModuleCategory::Modulation,
            ModuleCategory::Effect,
            ModuleCategory::Utility,
            ModuleCategory::Output,
        ];

        for category in categories {
            let data = SynthNodeData::new("test", "Test", category);
            // Verify the category is set correctly
            assert_eq!(data.category, category);
        }
    }

    #[test]
    fn test_node_data_clone() {
        let original = SynthNodeData::new("test", "Test Module", ModuleCategory::Utility);
        let cloned = original.clone();

        assert_eq!(original.module_id, cloned.module_id);
        assert_eq!(original.display_name, cloned.display_name);
        assert_eq!(original.category, cloned.category);
    }
}
