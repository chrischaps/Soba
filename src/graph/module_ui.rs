//! Per-module UI hints: the few things about a node that its DSP definition
//! doesn't say.
//!
//! Ports, parameters, ranges, defaults and names all come from the module's
//! `DspModule` definition (see [`super::catalog`]). This table only adds
//! presentation: short labels, knob order, which parameters stay hidden,
//! and which custom display a node draws. A module with no entry still gets
//! a working node, with a knob for every continuous parameter.

use super::node_data::NodeDisplay;

/// A knob in a node's bottom row.
#[derive(Clone, Copy, Debug)]
pub struct KnobHint {
    /// Parameter name, as declared by the module.
    pub param: &'static str,
    /// Short label under the knob.
    pub label: &'static str,
    /// When the parameter has a CV input: true if CV modulates around the
    /// knob, false if CV replaces it. Ignored for parameters without a CV input.
    pub modulatable: bool,
    /// A modulatable knob whose CV simply adds to it, so the knob can show
    /// where the CV has carried it.
    pub adds: bool,
}

/// A knob labeled with the parameter's own name.
const fn knob(param: &'static str) -> KnobHint {
    KnobHint { param, label: param, modulatable: false, adds: false }
}

/// A knob with a short label.
const fn knob_as(param: &'static str, label: &'static str) -> KnobHint {
    KnobHint { param, label, modulatable: false, adds: false }
}

/// A knob whose CV input modulates around the knob's value instead of replacing it.
const fn modulatable(param: &'static str, label: &'static str) -> KnobHint {
    KnobHint { param, label, modulatable: true, adds: false }
}

/// A modulatable knob whose CV adds to its value, clamped to its range: the
/// knob shows where the CV has carried it.
const fn added(param: &'static str, label: &'static str) -> KnobHint {
    KnobHint { param, label, modulatable: true, adds: true }
}

/// Presentation hints for one module.
#[derive(Debug)]
pub struct ModuleUi {
    pub module_id: &'static str,
    /// The knob row, in order. Every continuous parameter must be a knob or hidden.
    pub knobs: &'static [KnobHint],
    /// Short labels for inline toggles and dropdowns (parameter name → label).
    pub labels: &'static [(&'static str, &'static str)],
    /// Parameters with no control on the node. A trailing `*` matches a prefix.
    pub hidden: &'static [&'static str],
    /// Leading parameters driven by live input (the computer keyboard)
    /// rather than by the graph. Parameter sync leaves these alone.
    pub live_params: usize,
    /// Output ports the engine reports values for (lit gate ports, phase).
    pub monitor: &'static [&'static str],
    /// Custom visualization drawn above the knob row.
    pub display: NodeDisplay,
    /// Knobs per row before wrapping (0 keeps them all on one row).
    pub knobs_per_row: usize,
}

impl ModuleUi {
    const DEFAULT: ModuleUi = ModuleUi {
        module_id: "",
        knobs: &[],
        labels: &[],
        hidden: &[],
        live_params: 0,
        monitor: &[],
        display: NodeDisplay::None,
        knobs_per_row: 0,
    };

    /// Short inline label for a toggle or dropdown.
    pub fn label_for(&self, param: &str) -> Option<&'static str> {
        self.labels.iter().find(|(name, _)| *name == param).map(|(_, label)| *label)
    }

    /// Whether a parameter has no control on the node.
    pub fn is_hidden(&self, param: &str) -> bool {
        self.hidden.iter().any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => param.starts_with(prefix),
            None => param == *pattern,
        })
    }
}

/// Hints for a module, if it has an entry.
pub fn module_ui(module_id: &str) -> Option<&'static ModuleUi> {
    MODULE_UI.iter().find(|ui| ui.module_id == module_id)
}

static MODULE_UI: &[ModuleUi] = &[
    ModuleUi {
        module_id: "osc.sine",
        // Three rows: tune, timbre, ensemble
        knobs: &[
            knob_as("Octave", "Oct"),
            knob_as("Semitone", "Semi"),
            knob("Fine"),
            knob_as("FM Depth", "FM"),
            knob_as("Exp FM Depth", "Exp FM"),
            knob_as("Pulse Width", "PW"),
            knob("Voices"),
            knob("Detune"),
            knob("Spread"),
        ],
        labels: &[("Waveform", "Wave")],
        display: NodeDisplay::OscillatorWave,
        knobs_per_row: 3,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "source.noise",
        knobs: &[modulatable("Level", "Level"), modulatable("Rate", "Rate")],
        display: NodeDisplay::NoiseSpectrum,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "source.drum",
        knobs: &[
            modulatable("Tune", "Tune"),
            modulatable("Decay", "Decay"),
            knob("Tone"),
            knob("Snap"),
            knob("Level"),
        ],
        display: NodeDisplay::DrumHit,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "source.sampler",
        // What plays (the markers on the waveform), then how notes play
        // it (Slices beside Root, the key slice 1 is on), then the envelope
        // and level
        knobs: &[
            modulatable("Start", "Start"),
            knob("End"),
            knob_as("Loop Start", "Lp St"),
            knob_as("Loop End", "Lp End"),
            knob("Slices"),
            knob("Root"),
            knob("Tune"),
            knob("Fine"),
            modulatable("Speed", "Speed"),
            knob_as("Attack", "Atk"),
            knob_as("Release", "Rel"),
            knob("Level"),
            // Recording: how loud In must be before a take starts; and
            // slicing by Onsets: how strong a hit must be to get a marker
            knob_as("Rec Threshold", "Thresh"),
            knob_as("Sensitivity", "Sens"),
        ],
        hidden: &["Rec Button"],
        display: NodeDisplay::SamplerWave,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "source.chopper",
        // How long the loop is and how it's cut, then how loud
        knobs: &[knob("Beats"), knob("Slices"), knob_as("Sensitivity", "Sens"), knob("Level")],
        labels: &[("Pulse", "Pulse")],
        // The row of steps edits these
        hidden: &["Step *", "Chance *"],
        monitor: &["Gate"],
        display: NodeDisplay::ChopperRow,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "source.audio_input",
        knobs: &[knob("Gain"), knob_as("Threshold", "Thresh"), knob("Attack"), knob("Release")],
        monitor: &["Follow", "Gate"],
        display: NodeDisplay::InputListen,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "input.keyboard",
        knobs: &[knob_as("Octave", "Oct"), knob_as("Velocity", "Vel"), knob("Glide")],
        labels: &[("Glide Mode", "Glide")],
        hidden: &["Note", "Gate"],
        live_params: 2,
        monitor: &["Gate"],
        display: NodeDisplay::KeyboardPiano,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "input.midi_note",
        knobs: &[knob_as("Octave", "Oct"), knob_as("Bend Range", "Bend"), knob("Glide")],
        labels: &[("Channel", "Ch"), ("Retrigger", "Retrig"), ("Glide Mode", "Glide")],
        monitor: &["Gate"],
        display: NodeDisplay::MidiPiano,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "input.poly_midi",
        // How the voices play over their pitch offsets, two by two, so the
        // node stays narrow
        knobs: &[knob("Voices"), knob("Glide"), knob_as("Octave", "Oct"), knob_as("Bend Range", "Bend")],
        labels: &[("Channel", "Ch"), ("Allocation", "Mode"), ("Glide Mode", "Glide")],
        monitor: &["Gate"],
        display: NodeDisplay::MidiPiano,
        knobs_per_row: 2,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "filter.svf",
        // The CV depth beside Res, where a 303 puts Env Mod
        knobs: &[modulatable("Cutoff", "Cutoff"), modulatable("Resonance", "Res"), knob_as("Cutoff CV", "CV Amt"), knob("Drive")],
        display: NodeDisplay::FilterResponse,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "filter.ladder",
        // The CV depth beside Res, where a 303 puts Env Mod
        knobs: &[modulatable("Cutoff", "Cutoff"), modulatable("Resonance", "Res"), knob_as("Cutoff CV", "CV Amt"), knob("Drive")],
        display: NodeDisplay::LadderResponse,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "mod.adsr",
        // Two rows: times and level, then each stage's curve under its time
        // (velocity sits under sustain, both being about level)
        knobs: &[
            modulatable("Attack", "Atk"),
            modulatable("Decay", "Dec"),
            modulatable("Sustain", "Sus"),
            modulatable("Release", "Rel"),
            knob_as("Attack Curve", "A Crv"),
            knob_as("Decay Curve", "D Crv"),
            knob_as("Velocity Amount", "Vel"),
            knob_as("Release Curve", "R Crv"),
        ],
        display: NodeDisplay::Envelope,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "mod.lfo",
        knobs: &[modulatable("Rate", "Rate"), knob("Phase")],
        labels: &[("Waveform", "Wave"), ("Tempo Sync", "Tempo")],
        monitor: &["Phase"],
        display: NodeDisplay::LfoWave,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "mod.slope",
        knobs: &[modulatable("Rise", "Rise"), modulatable("Fall", "Fall"), knob("Shape")],
        monitor: &["EOR", "EOC"],
        display: NodeDisplay::SlopeShape,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.clock",
        knobs: &[modulatable("Tempo", "BPM"), knob_as("Gate Length", "Gate"), knob("Swing")],
        labels: &[("Division", "Div")],
        monitor: &["Gate", "Run", "Reset"],
        display: NodeDisplay::ClockBeat,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.vca",
        knobs: &[knob("Level"), knob_as("CV Amount", "CV Amt")],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.attenuverter",
        knobs: &[knob_as("Amount", "Amt"), knob("Offset")],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.mix",
        // One knob per input, in a row under their jacks
        knobs: &[knob_as("Level 1", "1"), knob_as("Level 2", "2"), knob_as("Level 3", "3"), knob_as("Level 4", "4")],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.mixer",
        // The display lays every knob out itself, a row per channel strip
        // (level, pan, width, send) and the bus row (return, master). The
        // short labels are what a group's face shows when one is pinned.
        // Every CV adds to its knob, which shows where the CV has carried it
        knobs: &[
            added("Level 1", "Lv 1"),
            added("Pan 1", "Pan 1"),
            knob_as("Width 1", "Wid 1"),
            added("Send 1", "Snd 1"),
            added("Level 2", "Lv 2"),
            added("Pan 2", "Pan 2"),
            knob_as("Width 2", "Wid 2"),
            added("Send 2", "Snd 2"),
            added("Level 3", "Lv 3"),
            added("Pan 3", "Pan 3"),
            knob_as("Width 3", "Wid 3"),
            added("Send 3", "Snd 3"),
            added("Level 4", "Lv 4"),
            added("Pan 4", "Pan 4"),
            knob_as("Width 4", "Wid 4"),
            added("Send 4", "Snd 4"),
            added("Return", "Ret"),
            knob("Master"),
        ],
        // Each strip's mute and solo are buttons at the end of its row
        hidden: &["Mute *", "Solo *"],
        display: NodeDisplay::MixerStrips,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.quantizer",
        knobs: &[modulatable("Transpose", "Transpose")],
        hidden: &["Mask"],
        monitor: &["Out", "Trig"],
        display: NodeDisplay::ScalePiano,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.divider",
        knobs: &[knob_as("Divide", "Div"), knob("Offset"), knob("Length")],
        monitor: &["Trig", "Gate"],
        display: NodeDisplay::DividerRing,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.logic",
        knobs: &[knob_as("Threshold", "Thresh")],
        // The lamps read the outputs, which light up too
        monitor: &["AND", "OR", "XOR", "NOT A", "Above"],
        display: NodeDisplay::LogicLamps,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.looper",
        knobs: &[knob_as("Feedback", "FB"), knob_as("Loop Level", "Loop"), knob_as("Dry Level", "Dry"), knob("Offset")],
        labels: &[("Latency", "Auto latency")],
        // The display's footswitches
        hidden: &["Pedal *"],
        monitor: &["Start"],
        display: NodeDisplay::LooperRing,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.oscilloscope",
        knobs: &[knob_as("Trigger Level", "Trig")],
        display: NodeDisplay::Scope,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "seq.step",
        knobs: &[knob("Steps"), knob_as("Gate Length", "Gate"), knob("Glide")],
        labels: &[("Direction", "Dir"), ("Gate Mode", "Gate of")],
        // The grid, the pattern tabs and the Chain edit these
        hidden: &["Step *", "Chain *"],
        // Step drives the grid's playhead, patched or not
        monitor: &["Gate", "Step", "EOC"],
        display: NodeDisplay::StepGrid,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "seq.chord",
        knobs: &[knob("Steps"), knob_as("Gate Length", "Gate"), knob("Voices"), knob("Range"), knob("Strum"), knob_as("Strum Taper", "Taper")],
        labels: &[("Direction", "Dir"), ("Voicing", "Voicing"), ("Voice Leading", "Voice lead"), ("Bass Voice", "Bass voice"), ("Rootless", "Rootless"), ("Strum Dir", "Strum dir")],
        // The grid and its popover edit these
        hidden: &["Step *"],
        monitor: &["Gate", "EOC"],
        display: NodeDisplay::ChordGrid,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "seq.trigger",
        knobs: &[knob("Steps"), knob_as("Gate Length", "Gate"), knob_as("Accent Amount", "Accent")],
        // The grid, the pattern tabs and the Chain edit these
        hidden: &["Step *", "Accent A *", "Accent B *", "Accent C *", "Accent D *", "Length *", "Chain *"],
        // The grid lights each lane as it plays
        monitor: &["Accent", "Gate 1", "Gate 2", "Gate 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"],
        display: NodeDisplay::TriggerGrid,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "seq.arranger",
        knobs: &[knob("Steps")],
        labels: &[("Loop", "Loop to")],
        // The timeline, its section bar and its menus edit these
        hidden: &["Sections", "Glide *", "Length *", "Section *", "Repeat *"],
        // The timeline lights each lane and trigger as it plays
        monitor: &[
            "Section Trig", "Bar", "Last Bar", "End",
            "Lane 1", "Gate 1", "Lane 2", "Gate 2", "Lane 3", "Gate 3", "Lane 4", "Gate 4",
            "Lane 5", "Gate 5", "Lane 6", "Gate 6", "Lane 7", "Gate 7", "Lane 8", "Gate 8",
        ],
        display: NodeDisplay::Timeline,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.delay",
        knobs: &[
            // CV swings Time by ±50% and adds to Feedback, around the knobs
            modulatable("Time", "Time"),
            modulatable("Feedback", "FB"),
            knob("Mix"),
            knob_as("High Cut", "HiCut"),
            knob_as("Low Cut", "LoCut"),
        ],
        labels: &[("Ping-Pong", "P-P")],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.reverb",
        knobs: &[
            // The room and its tail on top, placement in the mix below
            knob("Size"),
            knob("Decay"),
            knob_as("Damping", "Damp"),
            knob("Mod"),
            knob_as("Pre-Delay", "PreD"),
            knob("Width"),
            knob("Mix"),
        ],
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.eq",
        knobs: &[
            knob_as("Low Freq", "LoFrq"),
            knob_as("Low Gain", "LoGn"),
            knob_as("Mid Freq", "MdFrq"),
            knob_as("Mid Gain", "MdGn"),
            knob_as("Mid Q", "MdQ"),
            knob_as("High Freq", "HiFrq"),
            knob_as("High Gain", "HiGn"),
            knob_as("Output", "Out"),
        ],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.distortion",
        // The character of the curve on top, its colour and level below
        knobs: &[
            modulatable("Drive", "Drive"),
            knob_as("Symmetry", "Sym"),
            knob("Rate"),
            knob("Tone"),
            knob("Mix"),
            knob_as("Output", "Out"),
        ],
        knobs_per_row: 3,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.chorus",
        knobs: &[modulatable("Rate", "Rate"), modulatable("Depth", "Depth"), knob("Delay"), knob_as("Feedback", "FB"), knob("Mix")],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.tape",
        // The tape's colour on top, the transport's motion (and its stop) below
        knobs: &[
            knob_as("Saturation", "Sat"),
            knob("Age"),
            knob("Hiss"),
            knob("Mix"),
            modulatable("Wow", "Wow"),
            knob_as("Flutter", "Flut"),
            knob("Width"),
            knob_as("Stop Time", "Stop"),
        ],
        display: NodeDisplay::TapeReels,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.vocoder",
        // How the bank plays (the voice heard, the carrier evened), then what
        // passes besides, then the output
        knobs: &[
            knob("Attack"),
            knob("Release"),
            modulatable("Formant", "Formant"),
            knob("Flatten"),
            knob_as("Sibilance", "Sibil"),
            knob_as("Unvoiced", "Unvoic"),
            knob("Width"),
            knob("Mix"),
        ],
        display: NodeDisplay::VocoderBands,
        knobs_per_row: 4,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "fx.compressor",
        knobs: &[
            knob_as("Threshold", "Thresh"),
            knob("Ratio"),
            knob_as("Attack", "Atk"),
            knob_as("Release", "Rel"),
            knob("Knee"),
            knob_as("Makeup", "Mkup"),
            knob("Mix"),
        ],
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "util.midi_monitor",
        labels: &[("Channel", "Ch"), ("Pitch Bend", "PB")],
        display: NodeDisplay::MidiLog,
        ..ModuleUi::DEFAULT
    },
    ModuleUi {
        module_id: "output.audio",
        knobs: &[knob_as("Volume", "Vol")],
        display: NodeDisplay::OutputMeter,
        ..ModuleUi::DEFAULT
    },
];

/// Every entry in the table.
#[cfg(test)]
pub fn all() -> &'static [ModuleUi] {
    MODULE_UI
}

/// Hints used for a module with no entry in the table.
pub fn default_ui() -> &'static ModuleUi {
    &ModuleUi::DEFAULT
}
