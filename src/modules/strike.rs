//! Strike: one drum hit out to hardware (#484). A Kit has eight lanes, but
//! an SP-404MKII has sixteen pads a bank and a volca drum six parts. A
//! Strike is one lane on its own, so a kit is as many Strikes as the
//! device has parts.
//!
//! Patch a device's purple MIDI output (an Instrument, a Kit, a MIDI Out)
//! into a Strike's MIDI input and it plays that device: its Port and its
//! Channel follow the device's, and its part is one of the device's
//! profile's, chosen by name (Kick, Track 3, Pad 13). The cable is a
//! routing, as a MIDI In's is (#479): the editor follows it and copies the
//! device's Port, Channel and profile onto the Strike (see
//! `crate::graph::device_follow`). Unpatched, a Strike plays its own Port
//! and Channel, and its Note where it has no part.
//!
//! Its gate rising strikes the part: its note (or middle C, for a part
//! that's only a channel), on its channel (or the Strike's), as hard as
//! its Vel cable says at that sample, or the Velocity knob while it's
//! unpatched. Its gate falling lets the note go. Every message is stamped
//! with its sample, through its own ring to the sender, which sends each
//! when it's heard ([`MidiWriter`]).
//!
//! The part reaches the audio thread as a [`LaneMap`], by value, with
//! `EngineCommand::MapLane` for lane 0, as a Kit's lanes do.

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Transition},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::hardware::LaneMap;
use crate::link::midi::{Message, MidiBus, MIDI, PORTS, PORT_LABELS};
use crate::link::midi_writer::{MidiWriter, WRAP};
use crate::modules::midi_out::CHANNEL_LABELS;

/// The module's ID, as patches name it.
pub const ID: &str = "output.strike";

/// The name of a sender's MIDI input, and of a device's MIDI output: the
/// same as a MIDI In's, so the jack sits on the title bar.
pub const PORT_NAME: &str = crate::modules::midi_in::PORT_NAME;

/// The MIDI input a module that sends to a device has (#484): patched from
/// an Instrument, Kit or MIDI Out, it plays that device.
pub fn input() -> PortDefinition {
    PortDefinition::input("midi", PORT_NAME, SignalType::Midi)
        .describe("Patch from an Instrument, Kit or MIDI Out to play its device: its port, its channel and its profile's parts")
}

/// The MIDI output a device's node gains (#484), for Strikes and CC Outs to
/// be patched from.
pub fn output() -> PortDefinition {
    PortDefinition::output("midi", PORT_NAME, SignalType::Midi)
        .describe("Patch into a Strike's or CC Out's MIDI input, or a group's, and it plays this device, on its port and channel")
}

/// The note a Strike with no part plays to start with: the General MIDI kick.
pub const DEFAULT_NOTE: f32 = 36.0;

/// One drum hit, out to a device.
///
/// # Ports
///
/// **Inputs:**
/// - **MIDI** (MIDI): the device it plays, from an Instrument, Kit or MIDI
///   Out. Its jack sits on the title bar.
/// - **Gate** (Gate): strikes the part as it rises, and lets it go as it falls.
/// - **Vel** (Control): how hard, 0 to 1, read as the gate rises.
///
/// # Parameters
///
/// - **Port**, **Channel**: where it plays, the device's while patched. A
///   part without a channel of its own plays on the Channel.
/// - **Note**: the note it strikes while it has no part.
/// - **Velocity**: how hard it strikes while its Vel is unpatched.
pub struct Strike {
    out: MidiWriter,
    /// The port it played last block.
    port: usize,
    lane: LaneMap,
    gate: Edge,
    /// The note it holds, and the channel it went out on (from 0).
    holding: Option<(u8, u8)>,
    /// Its last strike, 0 to 1.
    velocity: f32,
    /// Seconds since it was last struck, for the face's flash.
    quiet: f32,
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// Notes sent so far, wrapping at [`WRAP`].
    notes_sent: u32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

/// The longest it's said to have been quiet, in seconds.
const QUIET: f32 = 60.0;

impl Strike {
    /// A Strike sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A Strike writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        Self {
            out: MidiWriter::on_bus(bus),
            port: 0,
            lane: LaneMap::EMPTY,
            gate: Edge::LOW,
            holding: None,
            velocity: 0.0,
            quiet: QUIET,
            was_playing: false,
            notes_sent: 0,
            ports: vec![
                input(),
                PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0)
                    .describe("Strikes its part as it rises, and lets it go as it falls: patch a Trigger Sequencer's lane here"),
                PortDefinition::input_with_default("vel", "Vel", SignalType::Control, 0.0)
                    .describe("How hard it strikes, 0 to 1, read as its gate rises. Unpatched, the Velocity knob says"),
            ],
            parameters: vec![
                ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                    .describe("Which device it plays: its device's port while its MIDI input is patched"),
                ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0)
                    .describe("The MIDI channel it plays where its part has none of its own: its device's while patched"),
                ParameterDefinition::new("note", "Note", 0.0, 127.0, DEFAULT_NOTE, ParameterDisplay::stepped(""))
                    .describe("The note it strikes while it plays no part of a profile's"),
                ParameterDefinition::new("velocity", "Velocity", 0.0, 1.0, 0.8, ParameterDisplay::linear("")).describe("How hard it strikes while its Vel jack is unpatched"),
            ],
        }
    }

    pub const PORT_MIDI: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VEL: usize = 2;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    pub const PARAM_NOTE: usize = 2;
    pub const PARAM_VELOCITY: usize = 3;

    // Readout slots

    /// Its last strike, 0 to 1, plus 2 while it's held.
    pub const READOUT_STRUCK: usize = 0;
    /// Seconds since it was last struck.
    pub const READOUT_QUIET: usize = 1;
    /// 1 when there's a sender to play to (the desktop app).
    pub const READOUT_LIVE: usize = 2;
    /// Notes sent so far, wrapping at [`WRAP`].
    pub const READOUT_NOTES_SENT: usize = 3;
    /// Messages the ring had no room for, wrapping at [`WRAP`].
    pub const READOUT_DROPPED: usize = 4;

    /// Its last strike (0 to 1), whether it's held, and the seconds since
    /// it was struck, from a readout.
    pub fn struck_in(readout: &Readout) -> (f32, bool, f32) {
        let struck = readout.values[Self::READOUT_STRUCK];
        let held = struck >= 2.0;
        (if held { struck - 2.0 } else { struck }, held, readout.values[Self::READOUT_QUIET])
    }

    /// What it plays now.
    pub fn lane(&self) -> LaneMap {
        self.lane
    }

    /// Lets go of its note, if it holds one.
    fn let_go(&mut self, offset: usize) {
        if let Some((note, channel)) = self.holding.take() {
            self.out.message(offset, Message::note_off(channel, note));
        }
    }
}

impl Default for Strike {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Strike {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Strike",
            category: ModuleCategory::Output,
            description: "One drum hit out to hardware over MIDI: patch it from an Instrument or Kit and it strikes one of the drum machine's or sampler's pads, tracks or parts, by name",
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
        let own_note = param(Self::PARAM_NOTE).round().clamp(0.0, 127.0) as u8;
        let velocity = param(Self::PARAM_VELOCITY);
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        self.out.begin(context, port);
        if port != self.port {
            // The sender lets go of the note on the old device
            self.holding = None;
            self.port = port;
        }
        let transport = &context.transport;
        let playing = transport.beat_position.is_some() && transport.playing;
        if self.was_playing && !playing {
            // Stopped mid-note: nothing will let go of it
            self.let_go(0);
        }
        self.was_playing = playing;
        self.quiet = (self.quiet + frames as f32 / context.sample_rate.max(1.0)).min(QUIET);

        let Some(gate) = connected_input(inputs, Self::PORT_GATE) else {
            self.gate.reset();
            self.let_go(0);
            self.out.end(frames);
            return;
        };
        let vel = connected_input(inputs, Self::PORT_VEL);
        // Its part's note and channel, or else its own
        let (note, on) = match self.lane {
            map if map.is_empty() => (own_note, channel),
            map => (map.note(), map.channel(channel)),
        };
        for offset in 0..frames {
            match self.gate.tick(gate.at(offset)) {
                None => {}
                Some(Transition::Falling) => self.let_go(offset),
                Some(Transition::Rising) => {
                    self.let_go(offset);
                    let strike = vel.map_or(velocity, |vel| vel.at(offset)).clamp(0.0, 1.0);
                    self.out.message(offset, Message::note_on(on, note, (strike * 127.0).round().clamp(1.0, 127.0) as u8));
                    self.holding = Some((note, on));
                    self.velocity = strike;
                    self.quiet = (frames - offset) as f32 / context.sample_rate.max(1.0);
                    self.notes_sent = (self.notes_sent + 1) % WRAP;
                }
            }
        }
        self.out.end(frames);
    }

    fn reset(&mut self) {
        self.let_go(0);
        self.gate = Edge::LOW;
    }

    fn map_lane(&mut self, lane: usize, map: LaneMap) {
        if lane != 0 {
            return;
        }
        // A note still sounding goes out where it came in
        if self.lane != map {
            self.let_go(0);
        }
        self.lane = map;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        values[Self::READOUT_STRUCK] = self.velocity + if self.holding.is_some() { 2.0 } else { 0.0 };
        values[Self::READOUT_QUIET] = self.quiet;
        values[Self::READOUT_LIVE] = if self.out.is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_NOTES_SENT] = self.notes_sent as f32;
        values[Self::READOUT_DROPPED] = self.out.dropped() as f32;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use crate::hardware::Part;
    use crate::link::midi::Outgoing;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    pub(crate) fn module() -> (Strike, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let module = Strike::on_bus(bus);
        let ring = bus.take_new_streams().pop().unwrap();
        (module, ring)
    }

    fn defaults(module: &Strike) -> Vec<f32> {
        module.parameters().iter().map(|p| p.default).collect()
    }

    fn cable(signal_type: SignalType, f: impl Fn(usize) -> f32) -> SignalBuffer {
        let mut buffer = SignalBuffer::polyphonic(BLOCK, signal_type);
        buffer.set_channels(1);
        for (i, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
            *sample = f(i);
        }
        buffer
    }

    fn inputs(gate: Option<SignalBuffer>, vel: Option<SignalBuffer>) -> Vec<SignalBuffer> {
        vec![
            SignalBuffer::unconnected(BLOCK, SignalType::Midi),
            gate.unwrap_or_else(|| SignalBuffer::unconnected(BLOCK, SignalType::Gate)),
            vel.unwrap_or_else(|| SignalBuffer::unconnected(BLOCK, SignalType::Control)),
        ]
    }

    fn hit_at(at: usize) -> SignalBuffer {
        cable(SignalType::Gate, move |i| if i >= at { 1.0 } else { 0.0 })
    }

    fn rest() -> SignalBuffer {
        cable(SignalType::Gate, |_| 0.0)
    }

    fn run(module: &mut Strike, inputs: &[SignalBuffer], params: &[f32]) {
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        module.process(&refs, &mut [], params, &ProcessContext::new(RATE, BLOCK));
    }

    fn messages(ring: &mut Consumer<Outgoing>) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(event) = ring.pop() {
            if let Outgoing::Message { sample, message } = event {
                out.push((sample, message.bytes().to_vec()));
            }
        }
        out
    }

    fn part(note: Option<u8>, channel: Option<u8>) -> Part {
        Part { name: "Part".into(), note, channel }
    }

    #[test]
    fn info_ports_and_parameters() {
        let (module, _) = module();
        assert_eq!(module.info().id, "output.strike");
        assert_eq!(module.ports()[Strike::PORT_MIDI].name, "MIDI", "first, as the editor numbers inputs first");
        assert_eq!(module.ports()[Strike::PORT_GATE].name, "Gate");
        assert_eq!(module.ports()[Strike::PORT_VEL].name, "Vel");
        assert_eq!(module.parameters()[Strike::PARAM_VELOCITY].name, "Velocity");
        assert_eq!(module.parameters().len(), Strike::PARAM_VELOCITY + 1);
        for word in ["drum machine", "sampler", "MIDI", "hardware"] {
            assert!(module.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn unpatched_it_plays_its_own_note_on_its_own_channel() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[Strike::PARAM_CHANNEL] = 9.0;
        run(&mut module, &inputs(Some(hit_at(10)), None), &params);
        assert_eq!(messages(&mut ring), [(10, vec![0x99, 36, 102])]);
        let (struck, held, quiet) = Strike::struck_in(&module.readout(&params).unwrap());
        assert!((struck - 0.8).abs() < 1e-6 && held && quiet < 0.002);
        run(&mut module, &inputs(Some(rest()), None), &params);
        assert_eq!(messages(&mut ring), [(64, vec![0x89, 36, 0])], "its gate falls at the top of the next block");
    }

    #[test]
    fn a_part_plays_its_note_on_its_channel_as_hard_as_vel_says() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        module.map_lane(0, LaneMap::of(&part(Some(2), Some(3)), true));
        let ramp = cable(SignalType::Control, |i| i as f32 / 63.0);
        run(&mut module, &inputs(Some(hit_at(20)), Some(ramp)), &params);
        let velocity = (20.0f32 / 63.0 * 127.0).round() as u8;
        assert_eq!(messages(&mut ring), [(20, vec![0x92, 2, velocity])]);
        // A volca drum's part: any note, on its channel
        module.map_lane(0, LaneMap::of(&part(None, Some(4)), false));
        assert_eq!(messages(&mut ring), [(64, vec![0x82, 2, 0])], "given another part, the old note lets go");
        run(&mut module, &inputs(Some(rest()), None), &params);
        run(&mut module, &inputs(Some(hit_at(5)), None), &params);
        assert_eq!(messages(&mut ring), [(133, vec![0x93, 60, 102])]);
    }

    #[test]
    fn struck_again_it_lets_go_first_and_unpatched_it_lets_go() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let twice = cable(SignalType::Gate, |i| if (4..8).contains(&i) || i >= 30 { 1.0 } else { 0.0 });
        run(&mut module, &inputs(Some(twice), None), &params);
        let bytes: Vec<_> = messages(&mut ring).into_iter().collect();
        assert_eq!(bytes, [(4, vec![0x90, 36, 102]), (8, vec![0x80, 36, 0]), (30, vec![0x90, 36, 102])]);
        run(&mut module, &inputs(None, None), &params);
        assert_eq!(messages(&mut ring), [(64, vec![0x80, 36, 0])]);
    }

    #[test]
    fn a_stopped_clock_lets_go_of_its_note() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let inputs = inputs(Some(hit_at(0)), None);
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut context = ProcessContext::new(RATE, BLOCK);
        context.transport = TransportState { beat_position: Some(0.0), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        context.transport = TransportState { playing: false, beat_position: Some(0.1), ..TransportState::playing_at(120.0) };
        module.process(&refs, &mut [], &params, &context);
        let bytes: Vec<_> = messages(&mut ring).into_iter().map(|(_, m)| m).collect();
        assert_eq!(bytes, [vec![0x90, 36, 102], vec![0x80, 36, 0]]);
    }

    #[test]
    fn strike_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Strike>();
    }
}
