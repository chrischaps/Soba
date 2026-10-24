//! Controls: a MIDI controller's faders, knobs and wheels as cables (#427).
//!
//! A mapping (#423) can only turn a knob that's already there. Controls
//! makes a fader a source instead: each of its four slots, A to D, hears one
//! control (a CC on a channel, the pitch bend, the mod wheel or the channel
//! pressure) and puts it out as a Control signal, to patch into a filter's
//! cutoff, a VCA, or through an attenuverter into an LFO's rate.
//!
//! It reads `ProcessContext.midi` on the audio thread, as Note In and the
//! Clock do, so a move lands on its own sample rather than waiting for a
//! UI frame. What each slot hears lives in its hidden parameters, which a
//! learn from the face writes (see [`crate::app::module_learn`]). An endless
//! encoder counts up and down inside the module, so a slot set to one
//! starts from where it is rather than jumping.

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
pub const ID: &str = "input.controls";

/// How many slots it has: A to D.
pub const SLOTS: usize = 4;

/// What a slot hears, as its Source parameter holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Source {
    /// A control change, of the slot's CC number.
    Cc = 0,
    /// The pitch bend wheel or strip, which rests in the middle.
    Bend = 1,
    /// The mod wheel: CC 1, whatever the slot's CC number says.
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
    fn rest(self) -> f32 {
        if self == Source::Bend { 0.5 } else { 0.0 }
    }

    /// Whether it swings either way of a middle: Auto Range makes it bipolar.
    fn centred(self) -> bool {
        self == Source::Bend
    }
}

/// How a slot's travel becomes its output, as its Range parameter holds it.
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

    /// Whether a slot hearing `source` swings from -1 to 1.
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

/// How far one step of an endless encoder moves a slot: 1% of its travel,
/// as a mapped knob steps (`midi_mappings::ENCODER_STEP`).
const ENCODER_STEP: f32 = 0.01;

/// How long a slot is slewed by default: enough to smooth a 7-bit CC's
/// steps out of a filter's cutoff, short enough not to feel late.
pub const DEFAULT_SLEW: f32 = 0.02;

/// The longest a slot is ever said to have been quiet, in seconds: the
/// readout's "never heard".
const QUIET: f32 = 1.0e6;

/// The Channel choices: 0 is Omni.
const CHANNEL_LABELS: &[&str] = &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// One slot's hidden parameters, by name, for A to D.
const SOURCE_NAMES: [(&str, &str); SLOTS] = [("source_a", "Source A"), ("source_b", "Source B"), ("source_c", "Source C"), ("source_d", "Source D")];
const CC_NAMES: [(&str, &str); SLOTS] = [("cc_a", "CC A"), ("cc_b", "CC B"), ("cc_c", "CC C"), ("cc_d", "CC D")];
const CHANNEL_NAMES: [(&str, &str); SLOTS] = [("channel_a", "Channel A"), ("channel_b", "Channel B"), ("channel_c", "Channel C"), ("channel_d", "Channel D")];
const ENCODING_NAMES: [(&str, &str); SLOTS] =
    [("encoding_a", "Encoding A"), ("encoding_b", "Encoding B"), ("encoding_c", "Encoding C"), ("encoding_d", "Encoding D")];
const RANGE_NAMES: [(&str, &str); SLOTS] = [("range_a", "Range A"), ("range_b", "Range B"), ("range_c", "Range C"), ("range_d", "Range D")];

/// What each slot hears when the module is added: the mod wheel, the pitch
/// bend, the pressure and CC 74 (brightness, and many controllers' first
/// knob), so a keyboard's own wheels work before anything is learned.
const DEFAULT_SOURCES: [(Source, u8); SLOTS] = [(Source::ModWheel, 1), (Source::Bend, 0), (Source::Pressure, 0), (Source::Cc, 74)];

/// A slot as it plays.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// The source it was hearing, so a new one starts from its own rest.
    source: Source,
    /// Where the control was last heard, 0 to 1: a CC's value over 127, a
    /// bend's from the bottom, an encoder's count.
    travel: f32,
    /// `travel` as slewed: what the output follows.
    slewed: f32,
    /// Seconds since it last heard its control.
    quiet: f32,
}

impl Slot {
    const IDLE: Slot = Slot { source: Source::Cc, travel: 0.0, slewed: 0.0, quiet: QUIET };
}

/// A slot's settings, read from the parameters once a block.
#[derive(Clone, Copy, Debug)]
struct Listen {
    source: Source,
    cc: u8,
    /// 0 is Omni, 1-16 a MIDI channel.
    channel: u8,
    encoding: MidiEncoding,
    bipolar: bool,
}

impl Listen {
    fn of(params: &[f32], slot: usize) -> Self {
        let at = |field: usize| params.get(Controls::param(slot, field)).copied().unwrap_or(0.0);
        let source = Source::from_param(at(FIELD_SOURCE));
        Self {
            source,
            cc: at(FIELD_CC).round().clamp(0.0, 127.0) as u8,
            channel: at(FIELD_CHANNEL).round().clamp(0.0, 16.0) as u8,
            encoding: encoding_from_param(at(FIELD_ENCODING)),
            bipolar: Range::from_param(at(FIELD_RANGE)).bipolar(source),
        }
    }

    /// Where `event` moves a slot at `travel` to, if it's the slot's control.
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

/// A slot's parameters, in the order each slot lists them.
const FIELD_SOURCE: usize = 0;
const FIELD_CC: usize = 1;
const FIELD_CHANNEL: usize = 2;
const FIELD_ENCODING: usize = 3;
const FIELD_RANGE: usize = 4;
const FIELDS: usize = 5;

/// Four controls from a MIDI controller, each as a Control output.
///
/// # Ports
///
/// **Outputs:**
/// - **A**, **B**, **C**, **D** (Control): each slot's control, 0 to 1, or
///   -1 to 1 when its range is bipolar.
///
/// # Parameters
///
/// - **Slew** (0-2 s): how long each output takes to follow its control.
/// - Per slot, hidden and set from the face: **Source** (CC, Pitch Bend,
///   Mod Wheel, Pressure), **CC** (0-127), **Channel** (Omni, 1-16),
///   **Encoding** (absolute, or an endless encoder's) and **Range** (Auto,
///   0 to 1, -1 to 1).
pub struct Controls {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    slots: [Slot; SLOTS],
    sample_rate: f32,
}

impl Controls {
    /// Creates a new Controls module.
    pub fn new() -> Self {
        let descriptions = [
            "Slot A's control: a fader, knob, wheel or strip from your MIDI controller",
            "Slot B's control",
            "Slot C's control",
            "Slot D's control",
        ];
        let ports = ["a", "b", "c", "d"]
            .iter()
            .zip(["A", "B", "C", "D"])
            .zip(descriptions)
            .map(|((&id, name), description)| PortDefinition::output(id, name, SignalType::Control).describe(description))
            .collect();
        let mut parameters = vec![ParameterDefinition::new("slew", "Slew", 0.0, 2.0, DEFAULT_SLEW, ParameterDisplay::logarithmic("s"))
            .describe("How long each output takes to follow its control; a little smooths a CC's steps")];
        for (slot, &(source, cc)) in DEFAULT_SOURCES.iter().enumerate() {
            parameters.extend([
                ParameterDefinition::choice(SOURCE_NAMES[slot].0, SOURCE_NAMES[slot].1, Source::LABELS, source as usize)
                    .describe("What the slot hears: a CC, the pitch bend, the mod wheel or channel pressure"),
                ParameterDefinition::new(CC_NAMES[slot].0, CC_NAMES[slot].1, 0.0, 127.0, cc as f32, ParameterDisplay::stepped(""))
                    .describe("The CC number the slot hears, when its source is a CC"),
                ParameterDefinition::choice(CHANNEL_NAMES[slot].0, CHANNEL_NAMES[slot].1, CHANNEL_LABELS, 0)
                    .describe("Only hear this MIDI channel; Omni hears all"),
                ParameterDefinition::choice(ENCODING_NAMES[slot].0, ENCODING_NAMES[slot].1, ENCODING_LABELS, 0)
                    .describe("A knob or fader with ends, or an endless encoder that sends steps"),
                ParameterDefinition::choice(RANGE_NAMES[slot].0, RANGE_NAMES[slot].1, Range::LABELS, 0)
                    .describe("0 to 1, or -1 to 1 about the middle; Auto is -1 to 1 for the pitch bend"),
            ]);
        }
        Self { ports, parameters, slots: [Slot::IDLE; SLOTS], sample_rate: 44100.0 }
    }

    /// The Slew parameter's index.
    pub const PARAM_SLEW: usize = 0;

    /// The index of a slot's parameter: its source, CC, channel, encoding
    /// or range ([`Controls::source_param`] and its neighbours name them).
    const fn param(slot: usize, field: usize) -> usize {
        1 + slot * FIELDS + field
    }

    /// The index of slot `slot`'s Source parameter.
    pub const fn source_param(slot: usize) -> usize {
        Self::param(slot, FIELD_SOURCE)
    }

    /// The index of slot `slot`'s CC parameter.
    pub const fn cc_param(slot: usize) -> usize {
        Self::param(slot, FIELD_CC)
    }

    /// The index of slot `slot`'s Channel parameter.
    pub const fn channel_param(slot: usize) -> usize {
        Self::param(slot, FIELD_CHANNEL)
    }

    /// The index of slot `slot`'s Encoding parameter.
    pub const fn encoding_param(slot: usize) -> usize {
        Self::param(slot, FIELD_ENCODING)
    }

    /// The index of slot `slot`'s Range parameter.
    pub const fn range_param(slot: usize) -> usize {
        Self::param(slot, FIELD_RANGE)
    }

    /// The readout's places: each slot's travel as slewed (0 to 1), then
    /// the seconds since each last heard its control.
    pub const READOUT_TRAVEL: usize = 0;
    pub const READOUT_QUIET: usize = SLOTS;

    /// A slot's letter.
    pub fn letter(slot: usize) -> &'static str {
        ["A", "B", "C", "D"].get(slot).copied().unwrap_or("?")
    }

    /// What a slot hears, in a word or two: "CC 74", "Bend", "Mod",
    /// "Pressure". CC 1 is the mod wheel, however it was chosen.
    pub fn source_label(source: Source, cc: u8) -> String {
        match source {
            Source::Cc if cc == 1 => "Mod".to_string(),
            Source::Cc => format!("CC {cc}"),
            Source::Bend => "Bend".to_string(),
            Source::ModWheel => "Mod".to_string(),
            Source::Pressure => "Press".to_string(),
        }
    }
}

impl Default for Controls {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Controls {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Controls",
            category: ModuleCategory::Source,
            description: "A MIDI controller's faders, knobs, mod wheel and pitch bend as cables, learned from the hardware",
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
        for (index, (slot, output)) in self.slots.iter_mut().zip(outputs.iter_mut()).enumerate() {
            let listen = Listen::of(params, index);
            // A new source starts from where its control rests
            if slot.source != listen.source {
                *slot = Slot { source: listen.source, travel: listen.source.rest(), slewed: listen.source.rest(), quiet: QUIET };
            }
            let mut events = context.midi.iter().peekable();
            let mut heard = false;
            for i in 0..context.block_size {
                // Everything that lands on this sample, in order
                while let Some(event) = events.next_if(|event| event.sample_offset as usize <= i) {
                    if let Some(travel) = listen.hear(event, slot.travel) {
                        slot.travel = travel;
                        heard = true;
                    }
                }
                slot.slewed += (slot.travel - slot.slewed) * rate;
                output.samples[i] = if listen.bipolar { slot.slewed * 2.0 - 1.0 } else { slot.slewed };
            }
            // Anything placed past the block still counts, from the next one
            for event in events {
                if let Some(travel) = listen.hear(event, slot.travel) {
                    slot.travel = travel;
                    heard = true;
                }
            }
            slot.quiet = if heard { 0.0 } else { (slot.quiet + block_seconds).min(QUIET) };
        }
    }

    fn reset(&mut self) {
        for slot in &mut self.slots {
            let rest = slot.source.rest();
            *slot = Slot { travel: rest, slewed: rest, quiet: QUIET, ..*slot };
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        for (index, slot) in self.slots.iter().enumerate() {
            readout.values[Self::READOUT_TRAVEL + index] = slot.slewed;
            readout.values[Self::READOUT_QUIET + index] = slot.quiet;
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;

    fn defaults() -> Vec<f32> {
        Controls::new().parameters().iter().map(|p| p.default).collect()
    }

    /// The defaults, with no slew and slot A on CC `cc` of `channel` (0 Omni).
    fn on_cc(cc: u8, channel: u8) -> Vec<f32> {
        let mut params = defaults();
        params[Controls::PARAM_SLEW] = 0.0;
        params[Controls::source_param(0)] = Source::Cc as u8 as f32;
        params[Controls::cc_param(0)] = cc as f32;
        params[Controls::channel_param(0)] = channel as f32;
        params
    }

    fn module() -> Controls {
        let mut module = Controls::new();
        module.prepare(SR, BLOCK);
        module
    }

    fn run(module: &mut Controls, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out: Vec<SignalBuffer> = (0..SLOTS).map(|_| SignalBuffer::control(BLOCK)).collect();
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    fn cc(at: u32, channel: u8, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::ControlChange { controller, value })
    }

    #[test]
    fn info_ports_and_parameters() {
        let controls = Controls::new();
        assert_eq!(controls.info().id, "input.controls");
        assert_eq!(controls.info().category, ModuleCategory::Source);
        let ports: Vec<&str> = controls.ports().iter().map(|p| p.name).collect();
        assert_eq!(ports, ["A", "B", "C", "D"]);
        assert!(controls.ports().iter().all(|p| p.is_output() && p.signal_type == SignalType::Control));
        let names: Vec<&str> = controls.parameters().iter().map(|p| p.name).collect();
        assert_eq!(names.len(), 1 + SLOTS * FIELDS);
        assert_eq!(names[Controls::source_param(2)], "Source C");
        assert_eq!(names[Controls::cc_param(3)], "CC D");
        assert_eq!(names[Controls::channel_param(1)], "Channel B");
        assert_eq!(names[Controls::encoding_param(0)], "Encoding A");
        assert_eq!(names[Controls::range_param(3)], "Range D");
        for word in ["fader", "knob", "mod wheel", "pitch bend", "MIDI"] {
            assert!(controls.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn a_cc_on_its_channel_moves_the_slot_on_its_sample_and_others_dont() {
        let mut module = module();
        let params = on_cc(74, 3);
        // Channel 3 is 2 on the wire; channel 1 and CC 75 are someone else's
        let out = run(&mut module, &params, &[cc(10, 2, 74, 127), cc(20, 0, 74, 0), cc(30, 2, 75, 0)]);
        assert_eq!(out[0].samples[9], 0.0);
        assert_eq!(out[0].samples[10], 1.0, "on its own sample");
        assert_eq!(out[0].samples[BLOCK - 1], 1.0);

        // Omni hears every channel, but never the computer keys
        let params = on_cc(74, 0);
        let out = run(&mut module, &params, &[cc(0, 9, 74, 0), cc(5, KEYS_CHANNEL, 74, 127)]);
        assert_eq!(out[0].samples[BLOCK - 1], 0.0);
        let out = run(&mut module, &params, &[cc(0, 15, 74, 127)]);
        assert_eq!(out[0].samples[0], 1.0);
    }

    #[test]
    fn the_default_slots_hear_the_wheels_and_pressure() {
        let mut module = module();
        let mut params = defaults();
        params[Controls::PARAM_SLEW] = 0.0;
        let out = run(&mut module, &params, &[]);
        // Before anything is heard, the bend rests in the middle: 0, bipolar
        assert_eq!([out[0].samples[0], out[1].samples[0], out[2].samples[0]], [0.0, 0.0, 0.0]);
        let midi = [
            cc(0, 0, 1, 127),
            MidiEvent::new(0, 0, MidiMessage::PitchBend { value: -8192 }),
            MidiEvent::new(0, 0, MidiMessage::Aftertouch { pressure: 127 }),
            cc(0, 0, 74, 127),
        ];
        let out = run(&mut module, &params, &midi);
        let last: Vec<f32> = out.iter().map(|o| o.samples[BLOCK - 1]).collect();
        assert_eq!(last, [1.0, -1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_bipolar_range_puts_the_middle_at_zero() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        params[Controls::range_param(0)] = 2.0;
        let at = |module: &mut Controls, value| run(module, &params, &[cc(0, 0, 20, value)])[0].samples[BLOCK - 1];
        assert_eq!(at(&mut module, 0), -1.0);
        assert_eq!(at(&mut module, 127), 1.0);
        assert!(at(&mut module, 64).abs() < 0.01);

        // The bend is bipolar on Auto, and unipolar when told
        params[Controls::source_param(0)] = Source::Bend as u8 as f32;
        params[Controls::range_param(0)] = 0.0;
        let bend = |value| [MidiEvent::new(0, 0, MidiMessage::PitchBend { value })];
        assert_eq!(run(&mut module, &params, &bend(0))[0].samples[0], 0.0);
        assert!((run(&mut module, &params, &bend(8191))[0].samples[0] - 1.0).abs() < 0.001);
        params[Controls::range_param(0)] = 1.0;
        assert_eq!(run(&mut module, &params, &bend(0))[0].samples[0], 0.5);
    }

    #[test]
    fn slew_glides_to_the_control_without_steps() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        params[Controls::PARAM_SLEW] = DEFAULT_SLEW;
        let out = run(&mut module, &params, &[cc(0, 0, 20, 127)]);
        let samples = &out[0].samples;
        assert!(samples[0] < 0.01, "it doesn't jump");
        assert!(samples.windows(2).all(|w| w[1] > w[0]), "it rises smoothly");
        // One time constant covers 63% of the way
        let tau = (DEFAULT_SLEW * SR) as usize;
        let mut after = samples[BLOCK - 1];
        for _ in 0..tau / BLOCK {
            after = run(&mut module, &params, &[])[0].samples[BLOCK - 1];
        }
        assert!((after - 0.63).abs() < 0.08, "{after}");
        // A long while later it's there
        for _ in 0..200 {
            after = run(&mut module, &params, &[])[0].samples[BLOCK - 1];
        }
        assert!((after - 1.0).abs() < 1e-3);
    }

    #[test]
    fn an_endless_encoder_counts_up_and_down_from_where_it_is() {
        let mut module = module();
        let mut params = on_cc(30, 0);
        params[Controls::encoding_param(0)] = encoding_param(MidiEncoding::Relative64);
        // Ten steps up, then three down: 1% of the travel each
        let mut midi: Vec<MidiEvent> = (0..10).map(|i| cc(i, 0, 30, 65)).collect();
        midi.push(cc(20, 0, 30, 61));
        let out = run(&mut module, &params, &midi);
        assert!((out[0].samples[BLOCK - 1] - 0.07).abs() < 1e-5);
        // It stops at the ends
        let down: Vec<MidiEvent> = (0..5).map(|i| cc(i, 0, 30, 0)).collect();
        assert_eq!(run(&mut module, &params, &down)[0].samples[BLOCK - 1], 0.0);

        // Two's complement: 1 up, 127 down
        params[Controls::encoding_param(0)] = encoding_param(MidiEncoding::TwosComplement);
        let out = run(&mut module, &params, &[cc(0, 0, 30, 5), cc(1, 0, 30, 127)]);
        assert!((out[0].samples[BLOCK - 1] - 0.04).abs() < 1e-5);
    }

    #[test]
    fn a_new_source_starts_from_its_rest_and_the_readout_follows() {
        let mut module = module();
        let mut params = on_cc(20, 0);
        run(&mut module, &params, &[cc(0, 0, 20, 127)]);
        let readout = module.readout(&params).unwrap();
        assert_eq!(readout.values[Controls::READOUT_TRAVEL], 1.0);
        assert_eq!(readout.values[Controls::READOUT_QUIET], 0.0, "just heard");
        run(&mut module, &params, &[]);
        let quiet = module.readout(&params).unwrap().values[Controls::READOUT_QUIET];
        assert!((quiet - BLOCK as f32 / SR).abs() < 1e-6);

        params[Controls::source_param(0)] = Source::Bend as u8 as f32;
        run(&mut module, &params, &[]);
        assert_eq!(module.readout(&params).unwrap().values[Controls::READOUT_TRAVEL], 0.5);
    }

    #[test]
    fn labels_name_the_source() {
        assert_eq!(Controls::source_label(Source::Cc, 74), "CC 74");
        assert_eq!(Controls::source_label(Source::Cc, 1), "Mod");
        assert_eq!(Controls::source_label(Source::Bend, 0), "Bend");
        assert_eq!(Controls::source_label(Source::ModWheel, 9), "Mod");
        assert_eq!(encoding_from_param(encoding_param(MidiEncoding::TwosComplement)), MidiEncoding::TwosComplement);
    }
}
