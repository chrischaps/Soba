//! Instrument: a piece of hardware as a node (#442). Choose a profile
//! (#441), a MicroFreak say, and its twelve knobs are the device's own:
//! Cutoff, Resonance, Timbre, Wave. Each has a jack beside it, so an LFO
//! patched into Cutoff sweeps the real filter, and the patch remembers
//! where every knob is. For a device that can't store a sound (a volca),
//! the patch becomes its preset memory: open it, and every knob is sent.
//!
//! It plays notes as MIDI Out does, from a gate and a pitch cable, with
//! bend and pressure, through the same send path (#439): every message is
//! stamped with its sample, and a port's budget thins the knobs' streams
//! before it holds back a note.
//!
//! The profile itself never reaches the audio thread. The editor boils
//! each knob's parameter down to a [`SlotMap`] (where it goes, across which
//! values, and a stepped one's exact values) and hands it over by value
//! with `EngineCommand::MapSlot`. A knob given a map sends its value again,
//! which is how **Send all** works: on opening the patch, on choosing a
//! profile, and from the face's menu. What each knob holds is kept on the
//! node (see [`crate::hardware::instrument`]).

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::hardware::SlotMap;
use crate::link::midi::{MidiBus, MIDI, PORTS, PORT_LABELS};
use crate::link::midi_player::Player;
use crate::link::midi_writer::{lane_gap, ControlLane, MidiWriter, WRAP};
use crate::modules::midi_out::{MidiOut, CHANNEL_LABELS};

/// The module's ID, as patches name it.
pub const ID: &str = "source.instrument";

/// How many knobs it has.
pub const SLOTS: usize = 12;

/// Each knob's name, as its parameter and its jack share it.
pub const SLOT_NAMES: [&str; SLOTS] = ["Slot 1", "Slot 2", "Slot 3", "Slot 4", "Slot 5", "Slot 6", "Slot 7", "Slot 8", "Slot 9", "Slot 10", "Slot 11", "Slot 12"];

/// A hardware synth, drum machine or sampler, played and turned from the
/// patch.
///
/// # Ports
///
/// **Inputs:**
/// - **Pitch** (Control, poly), **Gate** (Gate, poly), **Velocity**
///   (Control, poly): notes, as MIDI Out plays them.
/// - **Bend** (Control): -1 to 1, as Pitch Bend.
/// - **Pressure** (Control, poly): 0 to 1, as Channel or Poly Pressure.
/// - **Slot 1** … **Slot 12** (Control): 0 to 1, each across its knob's
///   travel, in place of the knob while patched.
///
/// # Parameters
///
/// - **Port**: A to H, as the Rig gives each a device.
/// - **Channel**: the MIDI channel it plays, and that knobs without a
///   channel of their own go out on.
/// - **Slot 1** … **Slot 12**: each knob's travel, 0 to 1, across what
///   the parameter it holds sends.
pub struct Instrument {
    out: MidiWriter,
    /// The port it played last block.
    port: usize,
    player: Player,
    knobs: Knobs,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

/// An Instrument's twelve knobs, as the audio thread sends them: a Kit's
/// too (#443). Each sends the parameter its [`SlotMap`] says, from its
/// cable where one's patched, wherever it moves.
pub struct Knobs {
    maps: [SlotMap; SLOTS],
    lanes: [ControlLane; SLOTS],
    /// A knob whose value counts as sent without sending it: just given a
    /// relative parameter, which sending would nudge, or (a Kit's) turned
    /// to another track, whose sound it would overwrite.
    settle: [bool; SLOTS],
    /// Knob values sent so far, wrapping at [`WRAP`], for the face's lamp.
    sent: u32,
}

impl Default for Knobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Knobs {
    pub const fn new() -> Self {
        Self { maps: [SlotMap::EMPTY; SLOTS], lanes: [ControlLane::new(); SLOTS], settle: [false; SLOTS], sent: 0 }
    }

    /// The knobs sent where they've moved: each from its jack (from
    /// `first_port`) while patched, or else its knob (from `first_param`),
    /// those without a channel of their own on `channel` (from 0).
    /// REAL-TIME SAFE.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, out: &mut MidiWriter, inputs: &[&SignalBuffer], first_port: usize, params: &[f32], first_param: usize, channel: u8, frames: usize, gap: usize) {
        for slot in 0..SLOTS {
            let map = self.maps[slot];
            let Some(address) = map.address else {
                self.lanes[slot].reset();
                continue;
            };
            let lane = &mut self.lanes[slot];
            let was = lane.sent();
            match connected_input(inputs, first_port + slot) {
                Some(cable) => {
                    if std::mem::take(&mut self.settle[slot]) {
                        lane.assume(map.control(address, channel, map.value(cable.at(0))));
                    }
                    lane.run(out, 0..frames, gap, |i| map.control(address, channel, map.value(cable.at(i))));
                }
                None => {
                    let travel = params.get(first_param + slot).copied().unwrap_or(0.0);
                    let control = map.control(address, channel, map.value(travel));
                    if std::mem::take(&mut self.settle[slot]) {
                        lane.assume(control);
                    }
                    if lane.sent() != Some(control) {
                        lane.run(out, 0..frames, gap, |_| control);
                    }
                }
            }
            if lane.sent() != was {
                self.sent = (self.sent + 1) % WRAP;
            }
        }
    }

    /// For a device or channel the knobs go to anew: each is sent afresh,
    /// but for those that nudge, which would move its sound.
    pub fn redirect(&mut self) {
        for (settle, map) in self.settle.iter_mut().zip(&self.maps) {
            *settle |= map.relative;
        }
    }

    /// For another track of the same device: no knob that's been sent is
    /// sent again until it's turned, so the track keeps its sound. One just
    /// given its parameter (Send all, a profile chosen) still goes.
    pub fn hold(&mut self) {
        for (settle, lane) in self.settle.iter_mut().zip(&self.lanes) {
            *settle |= lane.sent().is_some();
        }
    }

    /// Forgets what every knob sent, so each is sent again (but those
    /// settling).
    pub fn forget(&mut self) {
        self.lanes = [ControlLane::new(); SLOTS];
    }

    /// Gives knob `slot` what it sends, and has it send its value again,
    /// but for a relative one, which would nudge.
    pub fn map_slot(&mut self, slot: usize, map: SlotMap) {
        let Some(place) = self.maps.get_mut(slot) else { return };
        *place = map;
        self.lanes[slot].reset();
        self.settle[slot] = map.relative;
    }

    /// What knob `slot` holds now.
    pub fn map(&self, slot: usize) -> SlotMap {
        self.maps.get(slot).copied().unwrap_or_default()
    }

    /// Knob values sent so far, wrapping at [`WRAP`].
    pub fn sent(&self) -> u32 {
        self.sent
    }

    /// Each knob's value as last sent, into `values` from `at`, or -1
    /// while it holds nothing.
    pub fn read_into(&self, values: &mut [f32], at: usize) {
        for (slot, lane) in self.lanes.iter().enumerate() {
            values[at + slot] = match lane.sent().map(|control| control.value()) {
                Some(value) if !self.maps[slot].is_empty() => value as f32,
                _ => -1.0,
            };
        }
    }
}

impl Instrument {
    /// An Instrument sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// An Instrument writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        let mut ports = vec![
            PortDefinition::input_with_default("pitch", "Pitch", SignalType::Control, 0.0)
                .describe("V/Oct, 0 is C4 (note 60). A poly cable plays a chord, a note per channel"),
            PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0).describe("A note starts as each channel's gate rises and ends as it falls"),
            PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 0.8).describe("How hard each note is struck, 0 to 1, read as its gate rises"),
            PortDefinition::input_with_default("bend", "Bend", SignalType::Control, 0.0)
                .describe("-1 to 1, sent as Pitch Bend where it moves. Unplugged, it springs back to the middle"),
            PortDefinition::input_with_default("pressure", "Pressure", SignalType::Control, 0.0)
                .describe("0 to 1, sent as aftertouch: the channel's from a one-channel cable, each note's from a poly cable"),
        ];
        const SLOT_IDS: [&str; SLOTS] = ["slot1", "slot2", "slot3", "slot4", "slot5", "slot6", "slot7", "slot8", "slot9", "slot10", "slot11", "slot12"];
        ports.extend(SLOT_IDS.iter().zip(SLOT_NAMES).map(|(&id, name)| {
            PortDefinition::input_with_default(id, name, SignalType::Control, 0.0)
                .describe("0 to 1 across the knob's travel, sent to the hardware wherever it moves: an LFO here sweeps the real thing")
        }));
        let mut parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                .describe("Which device it plays: click the device on the face, or open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The MIDI channel it plays, and its knobs go out on"),
        ];
        parameters.extend(SLOT_IDS.iter().zip(SLOT_NAMES).map(|(&id, name)| {
            ParameterDefinition::new(id, name, 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                .describe("One of the hardware's own controls: right-click to choose which. Sent as it turns, and again with Send all")
        }));
        Self {
            out: MidiWriter::on_bus(bus),
            port: 0,
            player: Player::new(),
            knobs: Knobs::new(),
            ports,
            parameters,
        }
    }

    pub const PORT_PITCH: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VELOCITY: usize = 2;
    pub const PORT_BEND: usize = 3;
    pub const PORT_PRESSURE: usize = 4;
    /// The first knob's jack.
    pub const PORT_SLOT: usize = 5;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    /// The first knob.
    pub const PARAM_SLOT: usize = 2;

    // Readout slots

    /// Each knob's value as last sent, from this slot on, or -1 while it
    /// holds nothing.
    pub const READOUT_SENT: usize = 0;
    /// The notes held, 24 to a slot, from this slot on (six slots): read
    /// them with [`MidiOut::note_in`]'s layout, from here.
    pub const READOUT_NOTES: usize = SLOTS;
    /// 1 when there's a sender to play to (the desktop app).
    pub const READOUT_LIVE: usize = Self::READOUT_NOTES + 6;
    /// Notes sent so far, wrapping at [`WRAP`].
    pub const READOUT_NOTES_SENT: usize = Self::READOUT_LIVE + 1;
    /// Knob values sent so far, wrapping at [`WRAP`].
    pub const READOUT_KNOBS_SENT: usize = Self::READOUT_NOTES_SENT + 1;
    /// Messages the ring had no room for, wrapping at [`WRAP`].
    pub const READOUT_DROPPED: usize = Self::READOUT_KNOBS_SENT + 1;

    /// Whether note `note` is lit in a readout.
    pub fn note_in(readout: &Readout, note: u8) -> bool {
        let bits = readout.values[Self::READOUT_NOTES + note as usize / 24] as u32;
        bits & (1 << (note % 24)) != 0
    }

    /// What knob `slot` holds now.
    pub fn map(&self, slot: usize) -> SlotMap {
        self.knobs.map(slot)
    }
}

impl Default for Instrument {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Instrument {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Instrument",
            category: ModuleCategory::Output,
            description: "A piece of hardware as a node: a synth's or drum machine's own knobs, by name, to turn and patch, and its notes over MIDI",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn polyphonic(&self) -> bool {
        true
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block_size: usize) {}

    fn process(&mut self, inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let param = |index: usize| params.get(index).copied().unwrap_or(0.0);
        let frames = context.block_size;
        let gap = lane_gap(context.sample_rate);
        let port = (param(Self::PARAM_PORT).round().max(0.0) as usize).min(PORTS - 1);
        let channel = param(Self::PARAM_CHANNEL).round().clamp(0.0, 15.0) as u8;
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        self.out.begin(context, port);
        if port != self.port || channel != self.player.channel() {
            // The new device (or channel) hears every knob afresh, but for
            // those that nudge, which would move its sound
            self.knobs.redirect();
        }
        if port != self.port {
            self.knobs.forget();
            self.player.moved();
            self.port = port;
        }
        let transport = &context.transport;
        self.player.begin(&mut self.out, channel, transport.beat_position.is_some() && transport.playing);

        let input = |port: usize| connected_input(inputs, port);
        self.player.notes(&mut self.out, input(Self::PORT_GATE), input(Self::PORT_PITCH), input(Self::PORT_VELOCITY), frames);
        self.player.bend(&mut self.out, input(Self::PORT_BEND), frames, gap);
        self.player.pressure(&mut self.out, input(Self::PORT_PRESSURE), frames, gap);

        // The knobs: a cable where one's patched, wherever it moves
        self.knobs.run(&mut self.out, inputs, Self::PORT_SLOT, params, Self::PARAM_SLOT, channel, frames, gap);

        self.out.end(frames);
    }

    fn reset(&mut self) {
        self.player.reset(&mut self.out);
        self.knobs.forget();
    }

    fn map_slot(&mut self, slot: usize, map: SlotMap) {
        self.knobs.map_slot(slot, map);
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        self.knobs.read_into(values, Self::READOUT_SENT);
        MidiOut::light_notes(values, Self::READOUT_NOTES, &self.player);
        values[Self::READOUT_LIVE] = if self.out.is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_NOTES_SENT] = self.player.notes_sent() as f32;
        values[Self::READOUT_KNOBS_SENT] = self.knobs.sent() as f32;
        values[Self::READOUT_DROPPED] = self.out.dropped() as f32;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use crate::hardware::{Address, Parameter, Step};
    use crate::link::midi::Outgoing;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    fn module() -> (Instrument, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let module = Instrument::on_bus(bus);
        let ring = bus.take_new_streams().pop().unwrap();
        (module, ring)
    }

    fn defaults(module: &Instrument) -> Vec<f32> {
        module.parameters().iter().map(|p| p.default).collect()
    }

    fn inputs(module: &Instrument) -> Vec<SignalBuffer> {
        module.ports().iter().map(|port| SignalBuffer::unconnected(BLOCK, port.signal_type)).collect()
    }

    fn cable(signal_type: SignalType, f: impl Fn(usize) -> f32) -> SignalBuffer {
        let mut buffer = SignalBuffer::polyphonic(BLOCK, signal_type);
        buffer.set_channels(1);
        for (i, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = f(i);
        }
        buffer
    }

    fn run(module: &mut Instrument, inputs: &[SignalBuffer], params: &[f32]) {
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        module.process(&refs, &mut [], params, &ProcessContext::new(RATE, BLOCK));
    }

    /// The messages waiting, as (sample, bytes).
    fn messages(ring: &mut Consumer<Outgoing>) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(event) = ring.pop() {
            match event {
                Outgoing::Message { sample, message } => out.push((sample, message.bytes().to_vec())),
                Outgoing::Control { sample, control } => out.extend(control.burst().messages().iter().map(|m| (sample, m.bytes().to_vec()))),
                Outgoing::Block { .. } | Outgoing::Sysex { .. } => {}
            }
        }
        out
    }

    fn bytes(ring: &mut Consumer<Outgoing>) -> Vec<Vec<u8>> {
        messages(ring).into_iter().map(|(_, m)| m).collect()
    }

    fn cutoff() -> SlotMap {
        SlotMap::of(&Parameter::cc("Cutoff", 23))
    }

    #[test]
    fn info_ports_and_parameters() {
        let (module, _) = module();
        assert_eq!(module.info().id, "source.instrument");
        assert_eq!(module.info().category, ModuleCategory::Output);
        assert_eq!(module.ports()[Instrument::PORT_SLOT + 11].name, "Slot 12");
        assert_eq!(module.parameters()[Instrument::PARAM_SLOT + 11].name, "Slot 12");
        assert_eq!(module.parameters().len(), Instrument::PARAM_SLOT + SLOTS);
        const { assert!(Instrument::READOUT_DROPPED < crate::dsp::module_trait::MAX_READOUT) };
        for word in ["hardware", "synth", "drum machine", "MIDI"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn an_empty_knob_sends_nothing_and_a_mapped_one_sends_its_value() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "nothing mapped, nothing sent");
        module.map_slot(0, cutoff());
        params[Instrument::PARAM_SLOT] = 1.0;
        params[Instrument::PARAM_CHANNEL] = 3.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB3, 23, 127]]);
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "unchanged, unsent");
        // Turned, it goes again
        params[Instrument::PARAM_SLOT] = 0.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB3, 23, 0]]);
        let readout = module.readout(&params).unwrap();
        assert_eq!(readout.values[Instrument::READOUT_SENT], 0.0);
        assert_eq!(readout.values[Instrument::READOUT_SENT + 1], -1.0);
        assert_eq!(readout.values[Instrument::READOUT_KNOBS_SENT], 2.0);
    }

    #[test]
    fn mapping_a_knob_again_sends_it_again() {
        // Send all: the editor maps every knob afresh
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let inputs = inputs(&module);
        module.map_slot(4, cutoff());
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 23, 64]]);
        module.map_slot(4, cutoff());
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 23, 64]], "the same value, sent again");
        module.map_slot(SLOTS, cutoff());
        assert_eq!(module.map(SLOTS), SlotMap::EMPTY, "past the last knob, ignored");
    }

    #[test]
    fn a_patched_knob_sweeps_the_device_at_sample_timing() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        module.map_slot(0, cutoff());
        // A ramp: a millisecond (48 samples) apart at the most
        inputs[Instrument::PORT_SLOT] = cable(SignalType::Control, |i| i as f32 / 63.0);
        run(&mut module, &inputs, &params);
        let sent = messages(&mut ring);
        assert_eq!(sent.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [0, 48]);
        assert_eq!(sent[1].1, vec![0xB0, 23, (48.0f32 / 63.0 * 127.0).round() as u8]);
    }

    #[test]
    fn a_stepped_knob_sends_its_steps_own_values() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        let mut arp = Parameter::cc("Arp Type", 49);
        arp.values = [("Off", 6), ("Rise 1", 19), ("Fall 2", 65)].iter().map(|&(l, v)| Step::At { label: l.into(), value: v }).collect();
        module.map_slot(2, SlotMap::of(&arp));
        params[Instrument::PARAM_SLOT + 2] = 0.9;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 49, 65]]);
    }

    #[test]
    fn a_knob_on_its_own_channel_and_14_bit_pairs() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        let mut delay = Parameter::cc("Delay Time", 85);
        delay.channel = Some(9);
        module.map_slot(0, SlotMap::of(&delay));
        let mut depth = Parameter::cc("LFO1 Depth", 109);
        depth.address = Address::Cc14 { msb: 109, lsb: 61 };
        depth.range = (0, crate::hardware::profile::MAX_14);
        module.map_slot(1, SlotMap::of(&depth));
        params[Instrument::PARAM_SLOT] = 0.0;
        params[Instrument::PARAM_SLOT + 1] = 1.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB8, 85, 0], vec![0xB0, 109, 127], vec![0xB0, 61, 127]]);
    }

    #[test]
    fn a_relative_knob_sends_only_when_turned() {
        // The FANTOM's relative Cutoff nudges: mapping it mustn't
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        let mut nudge = Parameter::cc("Cutoff", 74);
        nudge.relative = true;
        module.map_slot(0, SlotMap::of(&nudge));
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "mapped, not nudged");
        params[Instrument::PARAM_SLOT] = 0.75;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 74, 95]]);
        // Another port, or channel, isn't nudged either
        params[Instrument::PARAM_PORT] = 1.0;
        run(&mut module, &inputs, &params);
        params[Instrument::PARAM_CHANNEL] = 4.0;
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty());
    }

    #[test]
    fn it_plays_notes_and_a_new_port_hears_every_knob_afresh() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let mut inputs = inputs(&module);
        module.map_slot(0, cutoff());
        inputs[Instrument::PORT_GATE] = cable(SignalType::Gate, |i| if i >= 10 { 1.0 } else { 0.0 });
        run(&mut module, &inputs, &params);
        let sent = messages(&mut ring);
        assert!(sent.contains(&(10, vec![0x90, 60, 102])), "{sent:?}");
        assert!(Instrument::note_in(&module.readout(&params).unwrap(), 60));
        params[Instrument::PARAM_PORT] = 3.0;
        inputs[Instrument::PORT_GATE] = cable(SignalType::Gate, |_| 1.0);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 23, 64]], "the knob again, for the new device");
    }

    #[test]
    fn a_stopped_clock_lets_go_of_its_notes() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        inputs[Instrument::PORT_GATE] = cable(SignalType::Gate, |_| 1.0);
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut context = ProcessContext::new(RATE, BLOCK);
        context.transport = TransportState { beat_position: Some(0.0), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        context.transport = TransportState { playing: false, beat_position: Some(0.1), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        assert_eq!(bytes(&mut ring), [vec![0x90, 60, 102], vec![0x80, 60, 0]]);
    }

    #[test]
    fn instrument_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Instrument>();
    }
}
