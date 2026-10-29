//! CC Out: one controller out to hardware (#484). MIDI Out has four CC
//! jacks and an Instrument twelve knobs; CC Outs are as many as you patch.
//!
//! Patch a device's purple MIDI output (an Instrument, a Kit, a MIDI Out)
//! into a CC Out's MIDI input and it plays that device: its Port and
//! Channel follow the device's, and it can send one of the device's
//! profile's parameters by name (Cutoff, Decay, Bus 1 Ctrl 3), in its own
//! CC, 14-bit pair or NRPN, across its own range. The cable is a routing,
//! as a Strike's is (see [`super::strike`]). Without a parameter it sends a
//! plain 7-bit CC, its CC number.
//!
//! The value is the In cable, 0 to 1, where one is patched, or else the
//! Value knob. It goes at the samples where what it sends changes, no more
//! often than once a millisecond ([`ControlLane`]), stamped with its
//! sample, through its own ring to the sender. A port's budget thins it
//! before it holds back a note.
//!
//! The parameter reaches the audio thread as a [`SlotMap`], by value, with
//! `EngineCommand::MapSlot` for slot 0, as an Instrument's knobs do.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::hardware::SlotMap;
use crate::link::midi::{CcMode, Control, MidiBus, MIDI, PORTS, PORT_LABELS};
use crate::link::midi_writer::{lane_gap, ControlLane, MidiWriter, WRAP};
use crate::modules::midi_out::CHANNEL_LABELS;

/// The module's ID, as patches name it.
pub const ID: &str = "output.cc_out";

/// The controller a new CC Out sends: the one most synths filter with.
pub const DEFAULT_CC: f32 = 74.0;

/// One controller, out to a device.
///
/// # Ports
///
/// **Inputs:**
/// - **MIDI** (MIDI): the device it plays, from an Instrument, Kit or MIDI
///   Out. Its jack sits on the title bar.
/// - **In** (Control): 0 to 1, in place of the Value knob while patched.
///
/// # Parameters
///
/// - **Port**, **Channel**: where it plays, the device's while patched. A
///   parameter with a channel of its own goes there.
/// - **CC**: the controller it sends while it holds no parameter.
/// - **Value**: 0 to 1, while In is unpatched.
pub struct CcOut {
    out: MidiWriter,
    /// The port it played last block.
    port: usize,
    map: SlotMap,
    lane: ControlLane,
    /// Its value counts as sent without sending it: it was just given a
    /// relative parameter, which sending would nudge.
    settle: bool,
    /// What it last sent, 0 to 1 across its travel, or -1 before anything.
    travel: f32,
    /// Values sent so far, wrapping at [`WRAP`].
    sent: u32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl CcOut {
    /// A CC Out sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A CC Out writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        Self {
            out: MidiWriter::on_bus(bus),
            port: 0,
            map: SlotMap::EMPTY,
            lane: ControlLane::new(),
            settle: false,
            travel: -1.0,
            sent: 0,
            ports: vec![
                super::strike::input(),
                PortDefinition::input_with_default("in", "In", SignalType::Control, 0.0)
                    .describe("0 to 1 across what it sends, sent to the hardware wherever it moves: an LFO here sweeps the real thing"),
            ],
            parameters: vec![
                ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                    .describe("Which device it plays: its device's port while its MIDI input is patched"),
                ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0)
                    .describe("The MIDI channel it sends on: its device's while patched"),
                ParameterDefinition::new("cc", "CC", 0.0, 127.0, DEFAULT_CC, ParameterDisplay::stepped(""))
                    .describe("The controller number it sends while it holds none of a profile's parameters"),
                ParameterDefinition::new("value", "Value", 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                    .describe("What it sends, 0 to 1 across its range, while In is unpatched"),
            ],
        }
    }

    pub const PORT_MIDI: usize = 0;
    pub const PORT_IN: usize = 1;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    pub const PARAM_CC: usize = 2;
    pub const PARAM_VALUE: usize = 3;

    // Readout slots

    /// What it last sent, 0 to 1 across its travel, or -1 before anything.
    pub const READOUT_TRAVEL: usize = 0;
    /// 1 when there's a sender to play to (the desktop app).
    pub const READOUT_LIVE: usize = 1;
    /// Values sent so far, wrapping at [`WRAP`].
    pub const READOUT_SENT: usize = 2;
    /// Messages the ring had no room for, wrapping at [`WRAP`].
    pub const READOUT_DROPPED: usize = 3;

    /// What it sends now.
    pub fn map(&self) -> SlotMap {
        self.map
    }

}

/// What `travel` (0 to 1) sends through `map`, on `channel` (from 0): its
/// parameter, or a plain CC `cc` for a map that holds none. REAL-TIME SAFE.
fn control(map: &SlotMap, travel: f32, channel: u8, cc: u8) -> Control {
    match map.address {
        Some(address) => map.control(address, channel, map.value(travel)),
        None => Control::lane(CcMode::Bits7, channel, cc, 0, travel),
    }
}

impl Default for CcOut {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for CcOut {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "CC Out",
            category: ModuleCategory::Output,
            description: "One controller out to hardware over MIDI: a CC, or one of the device's own knobs by name when patched from an Instrument or Kit, swept from a cable",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let param = |index: usize| params.get(index).copied().unwrap_or(0.0);
        let frames = context.block_size;
        let port = (param(Self::PARAM_PORT).round().max(0.0) as usize).min(PORTS - 1);
        let channel = param(Self::PARAM_CHANNEL).round().clamp(0.0, 15.0) as u8;
        let cc = param(Self::PARAM_CC).round().clamp(0.0, 127.0) as u8;
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        self.out.begin(context, port);
        if port != self.port {
            // Another device hears it afresh
            self.lane.reset();
            self.port = port;
        }
        let was = self.lane.sent();
        let cable = connected_input(inputs, Self::PORT_IN);
        let knob = param(Self::PARAM_VALUE);
        let travel = |i: usize| cable.map_or(knob, |cable| cable.at(i));
        let map = self.map;
        if std::mem::take(&mut self.settle) {
            self.lane.assume(control(&map, travel(0), channel, cc));
        }
        let gap = lane_gap(context.sample_rate);
        self.lane.run(&mut self.out, 0..frames, gap, |i| control(&map, travel(i), channel, cc));
        if self.lane.sent() != was {
            self.sent = (self.sent + 1) % WRAP;
            self.travel = travel(frames.saturating_sub(1)).clamp(0.0, 1.0);
        }
        self.out.end(frames);
    }

    fn reset(&mut self) {
        self.lane.reset();
    }

    fn map_slot(&mut self, slot: usize, map: SlotMap) {
        if slot != 0 {
            return;
        }
        // Given a parameter, it sends its value again, as a knob does: but
        // a relative one would nudge the device
        self.lane.reset();
        self.settle = map.relative;
        self.map = map;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        values[Self::READOUT_TRAVEL] = self.travel;
        values[Self::READOUT_LIVE] = if self.out.is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_SENT] = self.sent as f32;
        values[Self::READOUT_DROPPED] = self.out.dropped() as f32;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Address, Parameter};
    use crate::link::midi::Outgoing;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    fn module() -> (CcOut, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let module = CcOut::on_bus(bus);
        let ring = bus.take_new_streams().pop().unwrap();
        (module, ring)
    }

    fn defaults(module: &CcOut) -> Vec<f32> {
        module.parameters().iter().map(|p| p.default).collect()
    }

    fn inputs(cable: Option<SignalBuffer>) -> Vec<SignalBuffer> {
        vec![SignalBuffer::unconnected(BLOCK, SignalType::Midi), cable.unwrap_or_else(|| SignalBuffer::unconnected(BLOCK, SignalType::Control))]
    }

    fn cable(f: impl Fn(usize) -> f32) -> SignalBuffer {
        let mut buffer = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        buffer.set_channels(1);
        for (i, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = f(i);
        }
        buffer
    }

    fn run(module: &mut CcOut, inputs: &[SignalBuffer], params: &[f32]) {
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        module.process(&refs, &mut [], params, &ProcessContext::new(RATE, BLOCK));
    }

    fn bytes(ring: &mut Consumer<Outgoing>) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(event) = ring.pop() {
            if let Outgoing::Control { sample, control } = event {
                out.extend(control.burst().messages().iter().map(|m| (sample, m.bytes().to_vec())));
            }
        }
        out
    }

    #[test]
    fn info_ports_and_parameters() {
        let (module, _) = module();
        assert_eq!(module.info().id, "output.cc_out");
        assert_eq!(module.ports()[CcOut::PORT_MIDI].name, "MIDI");
        assert_eq!(module.ports()[CcOut::PORT_IN].name, "In");
        assert_eq!(module.parameters()[CcOut::PARAM_VALUE].name, "Value");
        assert_eq!(module.parameters().len(), CcOut::PARAM_VALUE + 1);
        for word in ["CC", "hardware", "MIDI"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn its_knob_sends_a_plain_cc_once_and_again_where_it_moves() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[CcOut::PARAM_CHANNEL] = 2.0;
        run(&mut module, &inputs(None), &params);
        assert_eq!(bytes(&mut ring), [(0, vec![0xB2, 74, 64])]);
        run(&mut module, &inputs(None), &params);
        assert!(bytes(&mut ring).is_empty(), "unchanged, nothing sent");
        params[CcOut::PARAM_VALUE] = 1.0;
        params[CcOut::PARAM_CC] = 20.0;
        run(&mut module, &inputs(None), &params);
        assert_eq!(bytes(&mut ring), [(128, vec![0xB2, 20, 127])]);
        let readout = module.readout(&params).unwrap();
        assert_eq!(readout.values[CcOut::READOUT_TRAVEL], 1.0);
    }

    #[test]
    fn a_cable_sweeps_it_no_faster_than_the_lane_gap() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        run(&mut module, &inputs(Some(cable(|i| i as f32 / 63.0))), &params);
        let sent = bytes(&mut ring);
        assert_eq!(sent.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [0, 48], "a millisecond apart at 48 kHz");
        assert_eq!(sent[1].1, vec![0xB0, 74, (48.0f32 / 63.0 * 127.0).round() as u8]);
    }

    #[test]
    fn a_profiles_parameter_goes_to_its_address_across_its_range() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[CcOut::PARAM_VALUE] = 1.0;
        let mut cutoff = Parameter::cc("Cutoff", 23);
        cutoff.range = (0, 100);
        module.map_slot(0, SlotMap::of(&cutoff));
        run(&mut module, &inputs(None), &params);
        assert_eq!(bytes(&mut ring), [(0, vec![0xB0, 23, 100])]);
        // An NRPN, given again, goes again
        let mut fine = Parameter::cc("Fine", 0);
        fine.address = Address::Nrpn(300);
        fine.range = (0, 16383);
        module.map_slot(0, SlotMap::of(&fine));
        run(&mut module, &inputs(None), &params);
        assert_eq!(bytes(&mut ring), [(64, vec![0xB0, 99, 2]), (64, vec![0xB0, 98, 44]), (64, vec![0xB0, 6, 127]), (64, vec![0xB0, 38, 127])]);
    }

    #[test]
    fn a_relative_parameter_given_isnt_nudged() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut nudge = Parameter::cc("Browse", 30);
        nudge.relative = true;
        module.map_slot(0, SlotMap::of(&nudge));
        run(&mut module, &inputs(None), &params);
        assert!(bytes(&mut ring).is_empty(), "the device is taken to be where it is");
    }

    #[test]
    fn cc_out_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<CcOut>();
    }
}
