//! Control: one fader, knob or wheel of a MIDI controller as a cable (#483).
//!
//! A mapping (#423) can only turn a knob that's already there. A Control
//! makes a fader a source instead: it hears one control (a CC on a
//! channel, the pitch bend, the mod wheel or the channel pressure) and puts
//! it out as a Control signal, to patch into a filter's cutoff, a VCA, or
//! through an attenuverter into an LFO's rate. A controller's four faders
//! are four Controls in a group: the Library's **MiniLab 3 Faders**, or a
//! plain **Controls ×4**. A lone expression pedal is one.
//!
//! It reads `ProcessContext.midi` on the audio thread, as Note In and the
//! Clock do, so a move lands on its own sample rather than waiting for a
//! UI frame. What it hears lives in its hidden parameters, which a learn
//! from the face writes (see [`crate::app::module_learn`]). An endless
//! encoder counts up and down inside the module, so a Control set to one
//! steps from where it stands rather than jumping.
//!
//! It took the place of Controls (#427), four of these in one node: a patch
//! that has one opens with a group of four Controls where it was (see
//! `crate::persistence::controls_group`).

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::rate_for_seconds,
    signal::SignalBuffer,
    MidiEvent, MidiMessage, ParameterDisplay, SignalType, KEYS_CHANNEL,
};
use crate::persistence::MidiEncoding;

/// The module's ID, as patches name it.
pub const ID: &str = "input.control";

/// What a Control hears, as its Source parameter holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Source {
    /// A control change, of its CC number.
    Cc = 0,
    /// The pitch bend wheel or strip, which rests in the middle.
    Bend = 1,
    /// The mod wheel: CC 1, whatever its CC number says.
    ModWheel = 2,
    /// Channel pressure: how hard a held key is pressed.
    Pressure = 3,
}

impl Source {
    /// The Source choices, in parameter order.
    pub const LABELS: &'static [&'static str] = &["CC", "Pitch Bend", "Mod Wheel", "Pressure"];
    const ALL: [Source; 4] = [Source::Cc, Source::Bend, Source::ModWheel, Source::Pressure];

    /// The source a Source parameter's value names.
    pub fn from_param(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    /// Where a control rests before it's heard, as a travel from 0 to 1: a
    /// bend in the middle, everything else at the bottom.
    pub fn rest(self) -> f32 {
        if self == Source::Bend { 0.5 } else { 0.0 }
    }

    /// Whether it swings either way of a middle: Auto Range makes it bipolar.
    fn centred(self) -> bool {
        self == Source::Bend
    }
}

/// How a Control's travel becomes its output, as its Range parameter holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// Bipolar for the pitch bend, unipolar for the rest.
    Auto,
    /// 0 to 1.
    Unipolar,
    /// -1 to 1, the middle of the travel at 0.
    Bipolar,
}

impl Range {
    /// The Range choices, in parameter order.
    pub const LABELS: &'static [&'static str] = &["Auto", "0 to 1", "-1 to 1"];
    const ALL: [Range; 3] = [Range::Auto, Range::Unipolar, Range::Bipolar];

    /// The range a Range parameter's value names.
    pub fn from_param(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }

    /// Whether a Control hearing `source` swings from -1 to 1.
    pub fn bipolar(self, source: Source) -> bool {
        match self {
            Range::Auto => source.centred(),
            Range::Unipolar => false,
            Range::Bipolar => true,
        }
    }
}

/// The Encoding choices, in parameter order: what a CC's values say.
pub const ENCODING_LABELS: &[&str] = &["Absolute", "Encoder (64 ±)", "Encoder (1 / 127)"];
const ENCODINGS: [MidiEncoding; 3] = [MidiEncoding::Absolute, MidiEncoding::Relative64, MidiEncoding::TwosComplement];

/// The encoding an Encoding parameter's value names.
pub fn encoding_from_param(value: f32) -> MidiEncoding {
    ENCODINGS[(value.round().max(0.0) as usize).min(ENCODINGS.len() - 1)]
}

/// The Encoding parameter's value for `encoding`.
pub fn encoding_param(encoding: MidiEncoding) -> f32 {
    ENCODINGS.iter().position(|&e| e == encoding).unwrap_or(0) as f32
}

/// How far one step of an endless encoder moves a Control: 1% of its
/// travel, as a mapped knob steps (`midi_mappings::ENCODER_STEP`).
const ENCODER_STEP: f32 = 0.01;

/// How long a Control is slewed by default: enough to smooth a 7-bit CC's
/// steps out of a filter's cutoff, short enough not to feel late.
pub const DEFAULT_SLEW: f32 = 0.02;

/// The longest a Control is ever said to have been quiet, in seconds: the
/// readout's "never heard".
const QUIET: f32 = 1.0e6;

/// The Channel choices: 0 is Omni.
const CHANNEL_LABELS: &[&str] = &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// What a Control hears, read from its parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listen {
    pub source: Source,
    pub cc: u8,
    /// 0 is Omni, 1-16 a MIDI channel.
    pub channel: u8,
    pub encoding: MidiEncoding,
    pub range: Range,
}

impl Listen {
    /// What a Control with `params` hears.
    pub fn of(params: &[f32]) -> Self {
        let at = |index: usize| params.get(index).copied().unwrap_or(0.0);
        Self {
            source: Source::from_param(at(Control::PARAM_SOURCE)),
            cc: at(Control::PARAM_CC).round().clamp(0.0, 127.0) as u8,
            channel: at(Control::PARAM_CHANNEL).round().clamp(0.0, 16.0) as u8,
            encoding: encoding_from_param(at(Control::PARAM_ENCODING)),
            range: Range::from_param(at(Control::PARAM_RANGE)),
        }
    }

    /// Whether it swings from -1 to 1.
    pub fn bipolar(&self) -> bool {
        self.range.bipolar(self.source)
    }

    /// What it hears, in a word or two: "CC 74", "Bend", "Mod", "Press".
    /// CC 1 is the mod wheel, however it was chosen.
    pub fn label(&self) -> String {
        match self.source {
            Source::Cc if self.cc == 1 => "Mod".to_string(),
            Source::Cc => format!("CC {}", self.cc),
            Source::Bend => "Bend".to_string(),
            Source::ModWheel => "Mod".to_string(),
            Source::Pressure => "Press".to_string(),
        }
    }

    /// Where `event` moves a Control at `travel` to, if it's its control.
    fn hear(&self, event: &MidiEvent, travel: f32) -> Option<f32> {
        // The computer keys have no controls to hear
        if event.channel == KEYS_CHANNEL || (self.channel != 0 && self.channel - 1 != event.channel) {
            return None;
        }
        match (self.source, event.message) {
            (Source::Cc, MidiMessage::ControlChange { controller, value }) if controller == self.cc => Some(match self.encoding.steps(value) {
                Some(steps) => (travel + steps as f32 * ENCODER_STEP).clamp(0.0, 1.0),
                None => value as f32 / 127.0,
            }),
            (Source::ModWheel, MidiMessage::ControlChange { controller: 1, value }) => Some(value as f32 / 127.0),
            (Source::Bend, MidiMessage::PitchBend { value }) => Some(((value as f32 / 8192.0).clamp(-1.0, 1.0) + 1.0) * 0.5),
            (Source::Pressure, MidiMessage::Aftertouch { pressure }) => Some(pressure as f32 / 127.0),
            _ => None,
        }
    }
}

/// One control from a MIDI controller, as a Control output.
///
/// # Ports
///
/// **Inputs:**
/// - **MIDI** (MIDI): from a MIDI In, the device and channel it hears
///   (#479); unpatched, its own Device.
///
/// **Outputs:**
/// - **Out** (Control): the control, 0 to 1, or -1 to 1 when its range is
///   bipolar.
///
/// # Parameters
///
/// - **Slew** (0-2 s): how long the output takes to follow its control.
/// - Hidden, set from the face: **Source** (CC, Pitch Bend, Mod Wheel,
///   Pressure), **CC** (0-127), **Channel** (Omni, 1-16), **Encoding**
///   (absolute, or an endless encoder's) and **Range** (Auto, 0 to 1, -1
///   to 1).
pub struct Control {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    /// The source it was hearing, so a new one starts from its own rest.
    source: Source,
    /// Where the control was last heard, 0 to 1: a CC's value over 127, a
    /// bend's from the bottom, an encoder's count.
    travel: f32,
    /// `travel` as slewed: what the output follows.
    slewed: f32,
    /// Seconds since it last heard its control.
    quiet: f32,
    sample_rate: f32,
}

impl Control {
    /// Creates a new Control, hearing the mod wheel.
    pub fn new() -> Self {
        // The device and channel it hears, from a MIDI In (#479), first:
        // the editor counts inputs before outputs
        let ports = vec![
            super::midi_in::input(),
            PortDefinition::output("out", "Out", SignalType::Control).describe("The control: a fader, knob, wheel or strip from your MIDI controller"),
        ];
        let parameters = vec![
            ParameterDefinition::choice("source", "Source", Source::LABELS, Source::ModWheel as usize)
                .describe("What it hears: a CC, the pitch bend, the mod wheel or channel pressure"),
            ParameterDefinition::new("cc", "CC", 0.0, 127.0, 1.0, ParameterDisplay::stepped("")).describe("The CC number it hears, when its source is a CC"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("Only hear this MIDI channel; Omni hears all"),
            ParameterDefinition::choice("encoding", "Encoding", ENCODING_LABELS, 0).describe("A knob or fader with ends, or an endless encoder that sends steps"),
            ParameterDefinition::choice("range", "Range", Range::LABELS, 0).describe("0 to 1, or -1 to 1 about the middle; Auto is -1 to 1 for the pitch bend"),
            ParameterDefinition::new("slew", "Slew", 0.0, 2.0, DEFAULT_SLEW, ParameterDisplay::logarithmic("s"))
                .describe("How long the output takes to follow its control; a little smooths a CC's steps"),
        ];
        let rest = Source::ModWheel.rest();
        Self { ports, parameters, source: Source::ModWheel, travel: rest, slewed: rest, quiet: QUIET, sample_rate: 44100.0 }
    }

    /// The parameters' indices.
    pub const PARAM_SOURCE: usize = 0;
    pub const PARAM_CC: usize = 1;
    pub const PARAM_CHANNEL: usize = 2;
    pub const PARAM_ENCODING: usize = 3;
    pub const PARAM_RANGE: usize = 4;
    pub const PARAM_SLEW: usize = 5;

    /// The readout's places: its travel as slewed (0 to 1), and the seconds
    /// since it last heard its control.
    pub const READOUT_TRAVEL: usize = 0;
    pub const READOUT_QUIET: usize = 1;

    /// Moves to where `event` puts the control, if it's this one's.
    #[inline]
    fn take(&mut self, listen: &Listen, event: &MidiEvent) -> bool {
        match listen.hear(event, self.travel) {
            Some(travel) => {
                self.travel = travel;
                true
            }
            None => false,
        }
    }
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Control {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Control",
            category: ModuleCategory::Source,
            description: "One fader, knob, mod wheel, pitch bend or pressure from a MIDI controller as a cable, learned from the hardware",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
    }

    fn process(&mut self, _inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let rate = rate_for_seconds(params.get(Self::PARAM_SLEW).copied().unwrap_or(DEFAULT_SLEW).max(0.0), self.sample_rate);
        let block_seconds = context.block_size as f32 / self.sample_rate;
        let listen = Listen::of(params);
        let bipolar = listen.bipolar();
        // A new source starts from where its control rests
        if self.source != listen.source {
            let rest = listen.source.rest();
            (self.source, self.travel, self.slewed, self.quiet) = (listen.source, rest, rest, QUIET);
        }
        let Some(output) = outputs.first_mut() else { return };
        let mut events = context.midi.iter().peekable();
        let mut heard = false;
        for i in 0..context.block_size {
            // Everything that lands on this sample, in order
            while let Some(event) = events.next_if(|event| event.sample_offset as usize <= i) {
                heard |= self.take(&listen, event);
            }
            self.slewed += (self.travel - self.slewed) * rate;
            output.samples[i] = if bipolar { self.slewed * 2.0 - 1.0 } else { self.slewed };
        }
        // Anything placed past the block still counts, from the next one
        for event in events {
            heard |= self.take(&listen, event);
        }
        self.quiet = if heard { 0.0 } else { (self.quiet + block_seconds).min(QUIET) };
    }

    fn reset(&mut self) {
        let rest = self.source.rest();
        (self.travel, self.slewed, self.quiet) = (rest, rest, QUIET);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_TRAVEL] = self.slewed;
        readout.values[Self::READOUT_QUIET] = self.quiet;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    fn defaults() -> Vec<f32> {
        Control::new().parameters().iter().map(|p| p.default).collect()
    }

    /// The defaults, with no slew, on CC `cc` of `channel` (0 Omni).
    fn on_cc(cc: u8, channel: u8) -> Vec<f32> {
        let mut params = defaults();
        params[Control::PARAM_SLEW] = 0.0;
        params[Control::PARAM_SOURCE] = Source::Cc as u8 as f32;
        params[Control::PARAM_CC] = cc as f32;
        params[Control::PARAM_CHANNEL] = channel as f32;
        params
    }

    fn module() -> Control {
        let mut module = Control::new();
        module.prepare(SR, BLOCK);
        module
    }

    /// The Control's output over one block.
    fn run(module: &mut Control, params: &[f32], midi: &[MidiEvent]) -> Vec<f32> {
        let mut out = [SignalBuffer::control(BLOCK)];
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out[0].samples.clone()
    }

    fn cc(at: u32, channel: u8, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::ControlChange { controller, value })
    }

    #[test]
    fn info_ports_and_parameters() {
        let control = Control::new();
        assert_eq!(control.info().id, "input.control");
        assert_eq!(control.info().category, ModuleCategory::Source);
        let ports: Vec<&str> = control.ports().iter().map(|p| p.name).collect();
        assert_eq!(ports, ["MIDI", "Out"], "the device it hears, from a MIDI In (#479), then its control");
        assert!(control.ports()[1].is_output() && control.ports()[1].signal_type == SignalType::Control);
        let names: Vec<&str> = control.parameters().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Source", "CC", "Channel", "Encoding", "Range", "Slew"]);
        for word in ["fader", "knob", "mod wheel", "pitch bend", "pressure", "MIDI"] {
            assert!(control.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn a_cc_on_its_channel_moves_it_on_its_sample_and_others_dont() {
        let mut module = module();
        let params = on_cc(74, 3);
        // Channel 3 is 2 on the wire; channel 1 and CC 75 are someone else's
        let out = run(&mut module, &params, &[cc(10, 2, 74, 127), cc(20, 0, 74, 0), cc(30, 2, 75, 0)]);
        assert_eq!(out[9], 0.0);
        assert_eq!(out[10], 1.0, "on its own sample");
        assert_eq!(out[BLOCK - 1], 1.0);

        // Omni hears every channel, but never the computer keys
        let params = on_cc(74, 0);
        let out = run(&mut module, &params, &[cc(0, 9, 74, 0), cc(5, KEYS_CHANNEL, 74, 127)]);
        assert_eq!(out[BLOCK - 1], 0.0);
        let out = run(&mut module, &params, &[cc(0, 15, 74, 127)]);
        assert_eq!(out[0], 1.0);
    }

    #[test]
    fn a_new_control_hears_the_mod_wheel_and_each_source_its_own() {
        let mut params = defaults();
        params[Control::PARAM_SLEW] = 0.0;
        assert_eq!(Listen::of(&params).label(), "Mod");
        let wheel = cc(0, 0, 1, 127);
        let bend = MidiEvent::new(0, 0, MidiMessage::PitchBend { value: -8192 });
        let pressure = MidiEvent::new(0, 0, MidiMessage::Aftertouch { pressure: 127 });
        let heard = |source: Source| {
            let mut module = module();
            let mut params = params.clone();
            params[Control::PARAM_SOURCE] = source as u8 as f32;
            // Before anything is heard, a bend rests in the middle: 0, bipolar
            let before = run(&mut module, &params, &[])[0];
            (before, run(&mut module, &params, &[wheel, bend, pressure])[BLOCK - 1])
        };
        assert_eq!(heard(Source::ModWheel), (0.0, 1.0));
        assert_eq!(heard(Source::Bend), (0.0, -1.0));
        assert_eq!(heard(Source::Pressure), (0.0, 1.0));
    }

    #[test]
    fn a_bipolar_range_puts_the_middle_at_zero() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        params[Control::PARAM_RANGE] = 2.0;
        let at = |module: &mut Control, value| run(module, &params, &[cc(0, 0, 20, value)])[BLOCK - 1];
        assert_eq!(at(&mut module, 0), -1.0);
        assert_eq!(at(&mut module, 127), 1.0);
        assert!(at(&mut module, 64).abs() < 0.01);

        // The bend is bipolar on Auto, and unipolar when told
        params[Control::PARAM_SOURCE] = Source::Bend as u8 as f32;
        params[Control::PARAM_RANGE] = 0.0;
        let bend = |value| [MidiEvent::new(0, 0, MidiMessage::PitchBend { value })];
        assert_eq!(run(&mut module, &params, &bend(0))[0], 0.0);
        assert!((run(&mut module, &params, &bend(8191))[0] - 1.0).abs() < 0.001);
        params[Control::PARAM_RANGE] = 1.0;
        assert_eq!(run(&mut module, &params, &bend(0))[0], 0.5);
    }

    #[test]
    fn slew_glides_to_the_control_without_steps() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        params[Control::PARAM_SLEW] = DEFAULT_SLEW;
        let samples = run(&mut module, &params, &[cc(0, 0, 20, 127)]);
        assert!(samples[0] < 0.01, "it doesn't jump");
        assert!(samples.windows(2).all(|w| w[1] > w[0]), "it rises smoothly");
        // One time constant covers 63% of the way
        let tau = (DEFAULT_SLEW * SR) as usize;
        let mut after = samples[BLOCK - 1];
        for _ in 0..tau / BLOCK {
            after = run(&mut module, &params, &[])[BLOCK - 1];
        }
        assert!((after - 0.63).abs() < 0.08, "{after}");
        // A long while later it's there
        for _ in 0..200 {
            after = run(&mut module, &params, &[])[BLOCK - 1];
        }
        assert!((after - 1.0).abs() < 1e-3);
    }

    #[test]
    fn an_endless_encoder_counts_up_and_down_from_where_it_is() {
        let mut module = module();
        let mut params = on_cc(30, 0);
        params[Control::PARAM_ENCODING] = encoding_param(MidiEncoding::Relative64);
        // Ten steps up, then three down: 1% of the travel each
        let mut midi: Vec<MidiEvent> = (0..10).map(|i| cc(i, 0, 30, 65)).collect();
        midi.push(cc(20, 0, 30, 61));
        assert!((run(&mut module, &params, &midi)[BLOCK - 1] - 0.07).abs() < 1e-5);
        // It stops at the ends
        let down: Vec<MidiEvent> = (0..5).map(|i| cc(i, 0, 30, 0)).collect();
        assert_eq!(run(&mut module, &params, &down)[BLOCK - 1], 0.0);

        // Two's complement: 1 up, 127 down
        params[Control::PARAM_ENCODING] = encoding_param(MidiEncoding::TwosComplement);
        assert!((run(&mut module, &params, &[cc(0, 0, 30, 5), cc(1, 0, 30, 127)])[BLOCK - 1] - 0.04).abs() < 1e-5);
    }

    #[test]
    fn on_cc_74_set_to_an_encoder_it_steps_from_where_it_stands() {
        // The acceptance (#483): heard as a fader to halfway, then told it's
        // an encoder, a step up moves it 1% from there, not from the bottom
        let mut module = module();
        let mut params = on_cc(74, 0);
        run(&mut module, &params, &[cc(0, 0, 74, 64)]);
        let stands = 64.0 / 127.0;
        params[Control::PARAM_ENCODING] = encoding_param(MidiEncoding::Relative64);
        assert_eq!(run(&mut module, &params, &[])[BLOCK - 1], stands, "telling it doesn't move it");
        let out = run(&mut module, &params, &[cc(0, 0, 74, 65), cc(1, 0, 74, 65)]);
        assert!((out[BLOCK - 1] - (stands + 0.02)).abs() < 1e-6, "{}", out[BLOCK - 1]);
        let out = run(&mut module, &params, &[cc(0, 0, 74, 63)]);
        assert!((out[BLOCK - 1] - (stands + 0.01)).abs() < 1e-6);
    }

    #[test]
    fn a_new_source_starts_from_its_rest_and_the_readout_follows() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        run(&mut module, &params, &[cc(0, 0, 20, 127)]);
        let readout = module.readout(&params).unwrap();
        assert_eq!(readout.values[Control::READOUT_TRAVEL], 1.0);
        assert_eq!(readout.values[Control::READOUT_QUIET], 0.0, "just heard");
        run(&mut module, &params, &[]);
        let quiet = module.readout(&params).unwrap().values[Control::READOUT_QUIET];
        assert!((quiet - BLOCK as f32 / SR).abs() < 1e-6);

        params[Control::PARAM_SOURCE] = Source::Bend as u8 as f32;
        run(&mut module, &params, &[]);
        assert_eq!(module.readout(&params).unwrap().values[Control::READOUT_TRAVEL], 0.5);
    }

    #[test]
    fn labels_name_the_source() {
        let listen = |source: Source, cc: u8| Listen { source, cc, channel: 0, encoding: MidiEncoding::Absolute, range: Range::Auto };
        assert_eq!(listen(Source::Cc, 74).label(), "CC 74");
        assert_eq!(listen(Source::Cc, 1).label(), "Mod");
        assert_eq!(listen(Source::Bend, 0).label(), "Bend");
        assert_eq!(listen(Source::ModWheel, 9).label(), "Mod");
        assert_eq!(encoding_from_param(encoding_param(MidiEncoding::TwosComplement)), MidiEncoding::TwosComplement);
    }
}
