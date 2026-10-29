//! Kit: a drum machine, sampler or groovebox as a node (#443). An
//! Instrument plays one voice; a Digitakt has a track a channel, an
//! SP-404MKII's pads are notes, a volca drum's parts each have a channel.
//! Choose a kit's profile and the node's eight lanes take its parts, by
//! name (Kick, Track 3, Pad 13), each a Gate and a Vel jack that patch
//! straight from a Trigger Sequencer's eight lanes.
//!
//! A lane's gate rising strikes its part: its note (or middle C, for a
//! part that's only a channel), on its channel (or the Kit's), as hard as
//! its Vel cable says at that sample, or the Velocity knob while it's
//! unpatched. Its gate falling lets the note go.
//!
//! Its twelve knobs are an Instrument's ([`Knobs`]). On a device that
//! takes the same CCs on each track's channel (the Digitakt), they turn the
//! lane the Track says. Choosing another track sends nothing until a knob
//! moves, so the track keeps its sound. A knob can keep a track of its own
//! whatever the Track says (#468): the editor hands it over already on that
//! track's channel, so twelve knobs hold a whole kit's sound.
//!
//! Which part each lane plays reaches the audio thread as a [`LaneMap`],
//! by value, with `EngineCommand::MapLane`, as the knobs' maps do.

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Transition},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::hardware::{LaneMap, SlotMap};
use crate::link::midi::{Message, MidiBus, MIDI, PORTS, PORT_LABELS};
use crate::link::midi_writer::{lane_gap, MidiWriter, WRAP};
use crate::modules::instrument::{Knobs, SLOTS, SLOT_NAMES};
use crate::modules::midi_out::CHANNEL_LABELS;

/// The module's ID, as patches name it.
pub const ID: &str = "source.kit";

/// How many lanes it has: a Trigger Sequencer's.
pub const LANES: usize = 8;

/// Each lane's gate jack, as a Trigger Sequencer's outputs are named.
pub const GATE_NAMES: [&str; LANES] = ["Gate 1", "Gate 2", "Gate 3", "Gate 4", "Gate 5", "Gate 6", "Gate 7", "Gate 8"];

/// Each lane's velocity jack.
pub const VEL_NAMES: [&str; LANES] = ["Vel 1", "Vel 2", "Vel 3", "Vel 4", "Vel 5", "Vel 6", "Vel 7", "Vel 8"];

/// The Track choice's values: which lane the knobs turn.
const TRACK_LABELS: [&str; LANES] = ["Lane 1", "Lane 2", "Lane 3", "Lane 4", "Lane 5", "Lane 6", "Lane 7", "Lane 8"];

/// A drum machine, sampler or groovebox, its parts struck from the patch.
///
/// # Ports
///
/// **Inputs:**
/// - **Gate 1** … **Gate 8** (Gate): each strikes its lane's part as it
///   rises, and lets it go as it falls.
/// - **Vel 1** … **Vel 8** (Control): how hard, 0 to 1, read as its gate
///   rises.
/// - **Slot 1** … **Slot 12** (Control): as an Instrument's.
///
/// # Parameters
///
/// - **Port**, **Channel**: as an Instrument's. Parts without a channel of
///   their own play on the Channel.
/// - **Track**: the lane whose track the knobs turn, where the device
///   takes them on each track's channel.
/// - **Velocity**: how hard a lane strikes while its Vel is unpatched.
/// - **Slot 1** … **Slot 12**: as an Instrument's.
pub struct Kit {
    out: MidiWriter,
    /// The port it played last block.
    port: usize,
    /// The channel the knobs went out on last block, from 0.
    knob_channel: u8,
    knobs: Knobs,
    lanes: [LaneMap; LANES],
    gates: [Edge; LANES],
    /// The note each lane holds, and the channel it went out on (from 0).
    holding: [Option<(u8, u8)>; LANES],
    /// Each lane's last strike, 0 to 1.
    velocity: [f32; LANES],
    /// Seconds since each lane was last struck, for the face's flash.
    quiet: [f32; LANES],
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// Notes sent so far, wrapping at [`WRAP`].
    notes_sent: u32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

/// The longest a lane is said to have been quiet, in seconds.
const QUIET: f32 = 60.0;

impl Kit {
    /// A Kit sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A Kit writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        const GATE_IDS: [&str; LANES] = ["gate_1", "gate_2", "gate_3", "gate_4", "gate_5", "gate_6", "gate_7", "gate_8"];
        const VEL_IDS: [&str; LANES] = ["vel_1", "vel_2", "vel_3", "vel_4", "vel_5", "vel_6", "vel_7", "vel_8"];
        const SLOT_IDS: [&str; SLOTS] = ["slot1", "slot2", "slot3", "slot4", "slot5", "slot6", "slot7", "slot8", "slot9", "slot10", "slot11", "slot12"];
        let mut ports = Vec::new();
        for lane in 0..LANES {
            ports.push(
                PortDefinition::input_with_default(GATE_IDS[lane], GATE_NAMES[lane], SignalType::Gate, 0.0)
                    .describe("Strikes this lane's part as it rises, and lets it go as it falls: patch a Trigger Sequencer's lane here"),
            );
            ports.push(
                PortDefinition::input_with_default(VEL_IDS[lane], VEL_NAMES[lane], SignalType::Control, 0.0)
                    .describe("How hard this lane strikes, 0 to 1, read as its gate rises. Unpatched, the Velocity knob says"),
            );
        }
        ports.extend(SLOT_IDS.iter().zip(SLOT_NAMES).map(|(&id, name)| {
            PortDefinition::input_with_default(id, name, SignalType::Control, 0.0)
                .describe("0 to 1 across the knob's travel, sent to the hardware wherever it moves: an LFO here sweeps the real thing")
        }));
        // Strikes and CC Outs patched from it play its device (#484)
        ports.push(crate::modules::strike::output());
        let mut parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                .describe("Which device it plays: click the device on the face, or open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0)
                .describe("The MIDI channel its parts play on where they have none of their own, and its knobs go out on"),
        ];
        parameters.extend(SLOT_IDS.iter().zip(SLOT_NAMES).map(|(&id, name)| {
            ParameterDefinition::new(id, name, 0.0, 1.0, 0.5, ParameterDisplay::linear(""))
                .describe("One of the hardware's own controls: right-click to choose which. Sent as it turns, and again with Send all")
        }));
        // After the knobs, whose jacks come first: neither has a row beside them
        parameters.push(
            ParameterDefinition::choice("track", "Track", &TRACK_LABELS, 0)
                .describe("The lane whose track the knobs turn, on a device that takes them on each track's channel: click a pad to choose"),
        );
        parameters.push(ParameterDefinition::new("velocity", "Velocity", 0.0, 1.0, 0.8, ParameterDisplay::linear("")).describe("How hard each lane strikes while its Vel jack is unpatched"));
        Self {
            out: MidiWriter::on_bus(bus),
            port: 0,
            knob_channel: 0,
            knobs: Knobs::new(),
            lanes: [LaneMap::EMPTY; LANES],
            gates: [Edge::LOW; LANES],
            holding: [None; LANES],
            velocity: [0.0; LANES],
            quiet: [QUIET; LANES],
            was_playing: false,
            notes_sent: 0,
            ports,
            parameters,
        }
    }

    /// Lane `lane`'s gate jack.
    pub const fn gate_port(lane: usize) -> usize {
        2 * lane
    }

    /// Lane `lane`'s velocity jack.
    pub const fn vel_port(lane: usize) -> usize {
        2 * lane + 1
    }

    /// The first knob's jack.
    pub const PORT_SLOT: usize = 2 * LANES;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    /// The first knob.
    pub const PARAM_SLOT: usize = 2;
    pub const PARAM_TRACK: usize = Self::PARAM_SLOT + SLOTS;
    pub const PARAM_VELOCITY: usize = Self::PARAM_TRACK + 1;

    // Readout slots

    /// Each knob's value as last sent, from this slot on, or -1 while it
    /// holds nothing.
    pub const READOUT_SENT: usize = 0;
    /// 1 when there's a sender to play to (the desktop app).
    pub const READOUT_LIVE: usize = SLOTS;
    /// Notes sent so far, wrapping at [`WRAP`].
    pub const READOUT_NOTES_SENT: usize = Self::READOUT_LIVE + 1;
    /// Knob values sent so far, wrapping at [`WRAP`].
    pub const READOUT_KNOBS_SENT: usize = Self::READOUT_NOTES_SENT + 1;
    /// Messages the ring had no room for, wrapping at [`WRAP`].
    pub const READOUT_DROPPED: usize = Self::READOUT_KNOBS_SENT + 1;
    /// Each lane's last strike, 0 to 1, plus 2 while it's held: read it
    /// with [`Kit::lane_in`].
    pub const READOUT_STRUCK: usize = Self::READOUT_DROPPED + 1;
    /// Seconds since each lane was last struck.
    pub const READOUT_QUIET: usize = Self::READOUT_STRUCK + LANES;

    /// Lane `lane` in a readout: its last strike (0 to 1), whether it's
    /// held, and the seconds since it was struck.
    pub fn lane_in(readout: &Readout, lane: usize) -> (f32, bool, f32) {
        let struck = readout.values[Self::READOUT_STRUCK + lane];
        let held = struck >= 2.0;
        (if held { struck - 2.0 } else { struck }, held, readout.values[Self::READOUT_QUIET + lane])
    }

    /// What knob `slot` holds now.
    pub fn map(&self, slot: usize) -> SlotMap {
        self.knobs.map(slot)
    }

    /// What lane `lane` plays now.
    pub fn lane(&self, lane: usize) -> LaneMap {
        self.lanes.get(lane).copied().unwrap_or_default()
    }

    /// Lets go of lane `lane`'s note, unless another lane holds it too.
    fn let_go(&mut self, lane: usize, offset: usize) {
        let Some(held) = self.holding[lane].take() else { return };
        if !self.holding.contains(&Some(held)) {
            let (note, channel) = held;
            self.out.message(offset, Message::note_off(channel, note));
        }
    }

    /// Lets go of every lane's note.
    fn let_all_go(&mut self, offset: usize) {
        for lane in 0..LANES {
            self.let_go(lane, offset);
        }
    }
}

impl Default for Kit {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Kit {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Kit",
            category: ModuleCategory::Output,
            description: "A hardware drum machine, sampler or groovebox as a node: its tracks, pads or parts as eight lanes to strike from a Trigger Sequencer, and its knobs by name, over MIDI",
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
        let gap = lane_gap(context.sample_rate);
        let port = (param(Self::PARAM_PORT).round().max(0.0) as usize).min(PORTS - 1);
        let channel = param(Self::PARAM_CHANNEL).round().clamp(0.0, 15.0) as u8;
        let track = (param(Self::PARAM_TRACK).round().max(0.0) as usize).min(LANES - 1);
        let velocity = param(Self::PARAM_VELOCITY);
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        self.out.begin(context, port);
        if port != self.port {
            // The sender lets go of the notes on the old device; the new
            // one hears every knob afresh
            self.holding = [None; LANES];
            self.knobs.redirect();
            self.knobs.forget();
            self.port = port;
        }
        let transport = &context.transport;
        let playing = transport.beat_position.is_some() && transport.playing;
        if self.was_playing && !playing {
            // Stopped mid-note: nothing will let go of it
            self.let_all_go(0);
        }
        self.was_playing = playing;

        // The lanes, sample by sample across them, so notes go in time order
        let lanes: [Option<&SignalBuffer>; LANES] = std::array::from_fn(|lane| connected_input(inputs, Self::gate_port(lane)));
        let block_seconds = frames as f32 / context.sample_rate.max(1.0);
        for lane in 0..LANES {
            if lanes[lane].is_none() {
                self.gates[lane].reset();
                self.let_go(lane, 0);
            }
            self.quiet[lane] = (self.quiet[lane] + block_seconds).min(QUIET);
        }
        for offset in 0..frames {
            for lane in 0..LANES {
                let Some(gate) = lanes[lane] else { continue };
                match self.gates[lane].tick(gate.at(offset)) {
                    None => {}
                    Some(Transition::Falling) => self.let_go(lane, offset),
                    Some(Transition::Rising) => {
                        let map = self.lanes[lane];
                        if map.is_empty() {
                            continue;
                        }
                        self.let_go(lane, offset);
                        let strike = connected_input(inputs, Self::vel_port(lane)).map_or(velocity, |vel| vel.at(offset));
                        let strike = strike.clamp(0.0, 1.0);
                        let (note, on) = (map.note(), map.channel(channel));
                        if self.holding.contains(&Some((note, on))) {
                            // Another lane holds it: struck again, it lets go first
                            self.out.message(offset, Message::note_off(on, note));
                        }
                        self.out.message(offset, Message::note_on(on, note, (strike * 127.0).round().clamp(1.0, 127.0) as u8));
                        self.holding[lane] = Some((note, on));
                        self.velocity[lane] = strike;
                        self.quiet[lane] = (frames - offset) as f32 / context.sample_rate.max(1.0);
                        self.notes_sent = (self.notes_sent + 1) % WRAP;
                    }
                }
            }
        }

        // The knobs, on the Track's channel where the device takes them so
        let knob_channel = match self.lanes[track] {
            map if map.track => map.channel(channel),
            _ => channel,
        };
        if knob_channel != self.knob_channel {
            // Another track: it keeps its sound until a knob is turned
            self.knobs.hold();
            self.knob_channel = knob_channel;
        }
        self.knobs.run(&mut self.out, inputs, Self::PORT_SLOT, params, Self::PARAM_SLOT, knob_channel, frames, gap);

        self.out.end(frames);
    }

    fn reset(&mut self) {
        self.let_all_go(0);
        self.gates = [Edge::LOW; LANES];
        self.knobs.forget();
    }

    fn map_slot(&mut self, slot: usize, map: SlotMap) {
        self.knobs.map_slot(slot, map);
    }

    fn slot_heard(&mut self, slot: usize, value: u16) {
        self.knobs.slot_heard(slot, value);
    }

    fn map_lane(&mut self, lane: usize, map: LaneMap) {
        if lane >= LANES {
            return;
        }
        // A note still sounding goes out where it came in
        if self.lanes[lane] != map {
            if let Some(held) = self.holding[lane].take() {
                if !self.holding.contains(&Some(held)) {
                    self.out.message(0, Message::note_off(held.1, held.0));
                }
            }
        }
        self.lanes[lane] = map;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        self.knobs.read_into(values, Self::READOUT_SENT);
        values[Self::READOUT_LIVE] = if self.out.is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_NOTES_SENT] = self.notes_sent as f32;
        values[Self::READOUT_KNOBS_SENT] = self.knobs.sent() as f32;
        values[Self::READOUT_DROPPED] = self.out.dropped() as f32;
        for lane in 0..LANES {
            values[Self::READOUT_STRUCK + lane] = self.velocity[lane] + if self.holding[lane].is_some() { 2.0 } else { 0.0 };
            values[Self::READOUT_QUIET + lane] = self.quiet[lane];
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use crate::hardware::{Parameter, Part};
    use crate::link::midi::Outgoing;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    fn module() -> (Kit, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let module = Kit::on_bus(bus);
        let ring = bus.take_new_streams().pop().unwrap();
        (module, ring)
    }

    fn defaults(module: &Kit) -> Vec<f32> {
        module.parameters().iter().map(|p| p.default).collect()
    }

    fn inputs(module: &Kit) -> Vec<SignalBuffer> {
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

    fn hit_at(at: usize) -> SignalBuffer {
        cable(SignalType::Gate, move |i| if i >= at { 1.0 } else { 0.0 })
    }

    fn held_down() -> SignalBuffer {
        cable(SignalType::Gate, |_| 1.0)
    }

    fn rest() -> SignalBuffer {
        cable(SignalType::Gate, |_| 0.0)
    }

    fn run(module: &mut Kit, inputs: &[SignalBuffer], params: &[f32]) {
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

    fn part(note: Option<u8>, channel: Option<u8>) -> Part {
        Part { name: "Part".into(), note, channel }
    }

    #[test]
    fn info_ports_and_parameters() {
        let (module, _) = module();
        assert_eq!(module.info().id, "source.kit");
        assert_eq!(module.ports()[Kit::gate_port(7)].name, "Gate 8");
        assert_eq!(module.ports()[Kit::vel_port(0)].name, "Vel 1");
        assert_eq!(module.ports()[Kit::PORT_SLOT + 11].name, "Slot 12");
        assert_eq!(module.parameters()[Kit::PARAM_VELOCITY].name, "Velocity");
        assert_eq!(module.parameters()[Kit::PARAM_SLOT + 11].name, "Slot 12");
        assert_eq!(module.parameters().len(), Kit::PARAM_VELOCITY + 1);
        const { assert!(Kit::READOUT_QUIET + LANES <= crate::dsp::module_trait::MAX_READOUT) };
        for word in ["drum machine", "sampler", "groovebox", "Trigger Sequencer", "MIDI"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn each_lane_strikes_its_part_and_lets_it_go() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[Kit::PARAM_CHANNEL] = 9.0;
        let mut inputs = inputs(&module);
        module.map_lane(0, LaneMap::of(&part(Some(36), None), false));
        module.map_lane(2, LaneMap::of(&part(Some(2), Some(3)), true));
        inputs[Kit::gate_port(0)] = hit_at(10);
        inputs[Kit::gate_port(2)] = hit_at(20);
        inputs[Kit::vel_port(2)] = cable(SignalType::Control, |i| i as f32 / 63.0);
        run(&mut module, &inputs, &params);
        let sent = messages(&mut ring);
        let velocity = (0.8f32 * 127.0).round() as u8;
        let ramp = (20.0f32 / 63.0 * 127.0).round() as u8;
        assert_eq!(sent, [(10, vec![0x99, 36, velocity]), (20, vec![0x92, 2, ramp])], "the Kit's channel, then the part's own");
        let (struck, held, quiet) = Kit::lane_in(&module.readout(&params).unwrap(), 2);
        assert!((struck - 20.0 / 63.0).abs() < 1e-6 && held && quiet < 0.001);
        inputs[Kit::gate_port(0)] = rest();
        inputs[Kit::gate_port(2)] = held_down();
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x89, 36, 0]]);
        // Unpatched, a lane lets go too
        inputs[Kit::gate_port(2)] = SignalBuffer::unconnected(BLOCK, SignalType::Gate);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x82, 2, 0]]);
    }

    #[test]
    fn an_empty_lane_and_a_channel_only_part() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        inputs[Kit::gate_port(1)] = hit_at(0);
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "a lane with no part plays nothing");
        // A volca drum's part: any note, on its channel
        module.map_lane(1, LaneMap::of(&part(None, Some(4)), true));
        inputs[Kit::gate_port(1)] = rest();
        run(&mut module, &inputs, &params);
        inputs[Kit::gate_port(1)] = hit_at(5);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x93, 60, 102]]);
    }

    #[test]
    fn two_lanes_on_one_part_let_go_with_the_last() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        let kick = LaneMap::of(&part(Some(36), None), false);
        module.map_lane(0, kick);
        module.map_lane(1, kick);
        inputs[Kit::gate_port(0)] = hit_at(0);
        inputs[Kit::gate_port(1)] = hit_at(8);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x90, 36, 102], vec![0x80, 36, 0], vec![0x90, 36, 102]], "struck again");
        inputs[Kit::gate_port(0)] = rest();
        inputs[Kit::gate_port(1)] = held_down();
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "lane 2 still holds it");
        inputs[Kit::gate_port(1)] = rest();
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x80, 36, 0]]);
    }

    #[test]
    fn knobs_turn_the_tracks_channel_and_another_track_keeps_its_sound() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        // The Digitakt's tracks: a channel each, the same CCs on each
        for lane in 0..LANES {
            module.map_lane(lane, LaneMap::of(&part(Some(lane as u8), Some(lane as u8 + 1)), true));
        }
        module.map_slot(0, SlotMap::of(&Parameter::cc("Filter Freq", 74)));
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 74, 64]], "given its parameter, it goes to track 1");
        params[Kit::PARAM_TRACK] = 2.0;
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "turned to track 3: it keeps its sound");
        params[Kit::PARAM_SLOT] = 1.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB2, 74, 127]], "turned, on track 3's channel");
        // Send all sends it there
        module.map_slot(0, SlotMap::of(&Parameter::cc("Filter Freq", 74)));
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB2, 74, 127]]);
        params[Kit::PARAM_TRACK] = 0.0;
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty(), "back to track 1, nothing sent");
    }

    #[test]
    fn knobs_stay_on_the_kits_channel_where_the_device_takes_them_so() {
        // The SP-404MKII's pads are notes: its knobs ignore the Track
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        module.map_lane(3, LaneMap::of(&part(Some(51), Some(1)), false));
        module.map_slot(0, SlotMap::of(&Parameter::cc("Ctrl 1", 16)));
        params[Kit::PARAM_TRACK] = 3.0;
        params[Kit::PARAM_CHANNEL] = 4.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB4, 16, 64]]);
    }

    #[test]
    fn a_lane_given_another_part_lets_go_of_the_old_one() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        module.map_lane(0, LaneMap::of(&part(Some(36), None), false));
        inputs[Kit::gate_port(0)] = hit_at(0);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x90, 36, 102]]);
        module.map_lane(0, LaneMap::of(&part(Some(38), None), false));
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0x80, 36, 0]], "the old pad lets go, and the new waits for the next hit");
        assert_eq!(module.lane(0).note(), 38);
    }

    #[test]
    fn a_stopped_clock_lets_go_of_its_notes() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        module.map_lane(0, LaneMap::of(&part(Some(36), None), false));
        inputs[Kit::gate_port(0)] = hit_at(0);
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut context = ProcessContext::new(RATE, BLOCK);
        context.transport = TransportState { beat_position: Some(0.0), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        context.transport = TransportState { playing: false, beat_position: Some(0.1), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        assert_eq!(bytes(&mut ring), [vec![0x90, 36, 102], vec![0x80, 36, 0]]);
    }

    /// Each lane of a Kit playing the bundled `profile`, its gate rising
    /// with the next: what goes out, and the knobs' channel with the Track
    /// on its last lane.
    fn played(profile: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let profiles = crate::hardware::Profiles::load_from(std::path::Path::new("no such folder"));
        let chosen = &profiles.find(profile).unwrap().profile;
        assert!(crate::hardware::instrument::is_kit(chosen), "{profile} is a kit");
        let labels = crate::hardware::instrument::choose_kit(chosen).into_iter().filter(|(_, text)| !text.is_empty()).collect();
        let (mut module, mut ring) = module();
        for (lane, map) in crate::hardware::instrument::lane_maps(&labels, &profiles).into_iter().enumerate() {
            module.map_lane(lane, map);
        }
        let mut params = defaults(&module);
        params[Kit::PARAM_VELOCITY] = 1.0;
        let mut inputs = inputs(&module);
        for lane in 0..LANES {
            inputs[Kit::gate_port(lane)] = hit_at(lane * 4);
        }
        run(&mut module, &inputs, &params);
        let notes = bytes(&mut ring);
        for lane in 0..LANES {
            inputs[Kit::gate_port(lane)] = held_down();
        }
        // A knob, turned on the last lane's track
        module.map_slot(0, SlotMap::of(&Parameter::cc("Knob", 20)));
        params[Kit::PARAM_TRACK] = (LANES - 1) as f32;
        run(&mut module, &inputs, &params);
        (notes, bytes(&mut ring))
    }

    #[test]
    fn a_digitakt_kit_sends_each_lane_on_its_tracks_channel() {
        // Elektron's manual, 8.4: notes 0-7 trigger tracks 1-8 on their channels 1-8
        let (notes, knobs) = played("Digitakt");
        let expected: Vec<Vec<u8>> = (0..8u8).map(|track| vec![0x90 + track, track, 127]).collect();
        assert_eq!(notes, expected);
        assert_eq!(knobs, [vec![0xB7, 20, 64]], "the knobs turn track 8, on its channel");
    }

    #[test]
    fn an_sp404_kit_sends_each_lane_as_its_pads_note() {
        // Roland's MIDI note map, mode A, bank A: pads 1-4 are 48-51, 5-8 are 44-47
        let (notes, knobs) = played("SP-404MKII");
        let expected: Vec<Vec<u8>> = [48, 49, 50, 51, 44, 45, 46, 47].iter().map(|&note| vec![0x90, note, 127]).collect();
        assert_eq!(notes, expected);
        assert_eq!(knobs, [vec![0xB0, 20, 64]], "its knobs stay on the Kit's channel");
    }

    #[test]
    fn a_volca_drum_kit_strikes_each_part_on_its_channel() {
        // Korg's split-channel chart: parts 1-6 on channels 1-6, any note
        let (notes, knobs) = played("volca drum");
        let expected: Vec<Vec<u8>> = (0..6u8).map(|part| vec![0x90 + part, 60, 127]).collect();
        assert_eq!(notes, expected, "six parts: lanes 7 and 8 play nothing");
        assert_eq!(knobs, [vec![0xB0, 20, 64]], "an empty lane has no track: the Kit's channel");
    }

    #[test]
    fn a_digitakt_kits_knobs_each_turn_their_own_track() {
        // Knob 1 on Track 1's Decay, knob 2 on Track 3's, knob 3 the Track's
        let profiles = crate::hardware::Profiles::load_from(std::path::Path::new("no such folder"));
        let digitakt = &profiles.find("Digitakt").unwrap().profile;
        let decay = crate::hardware::instrument::written(digitakt.parameter("Decay").unwrap());
        let mut labels: std::collections::BTreeMap<String, String> =
            crate::hardware::instrument::choose_kit(digitakt).into_iter().filter(|(_, text)| !text.is_empty()).collect();
        for slot in 0..SLOTS {
            labels.remove(&crate::hardware::instrument::slot_key(slot));
        }
        for (slot, track) in [(0, Some(0)), (1, Some(2)), (2, None)] {
            labels.insert(crate::hardware::instrument::slot_key(slot), decay.clone());
            if let Some(track) = track {
                labels.insert(crate::hardware::instrument::slot_track_key(slot), crate::hardware::instrument::written_part(&digitakt.parts[track]));
            }
        }
        let (mut module, mut ring) = module();
        for (lane, map) in crate::hardware::instrument::lane_maps(&labels, &profiles).into_iter().enumerate() {
            module.map_lane(lane, map);
        }
        let send_all = |module: &mut Kit| {
            for (slot, map) in crate::hardware::instrument::maps(&labels, &profiles).into_iter().enumerate() {
                module.map_slot(slot, map);
            }
        };
        send_all(&mut module);
        let mut params = defaults(&module);
        params[Kit::PARAM_TRACK] = 7.0;
        let inputs = inputs(&module);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 80, 64], vec![0xB2, 80, 64], vec![0xB7, 80, 64]], "each on its track's channel, the third on the Track's");
        // Another Track keeps every track's sound, and the knob that
        // follows it turns track 5
        params[Kit::PARAM_TRACK] = 4.0;
        run(&mut module, &inputs, &params);
        assert!(messages(&mut ring).is_empty());
        params[Kit::PARAM_SLOT] = 0.0;
        params[Kit::PARAM_SLOT + 1] = 1.0;
        params[Kit::PARAM_SLOT + 2] = 0.0;
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 80, 0], vec![0xB2, 80, 127], vec![0xB4, 80, 0]]);
        // Send all sends each to its own track again
        send_all(&mut module);
        run(&mut module, &inputs, &params);
        assert_eq!(bytes(&mut ring), [vec![0xB0, 80, 0], vec![0xB2, 80, 127], vec![0xB4, 80, 0]]);
    }

    #[test]
    fn kit_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Kit>();
    }
}
