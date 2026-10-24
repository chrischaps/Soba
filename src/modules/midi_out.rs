//! MIDI Out module: plays hardware and other apps from the patch.
//!
//! A gate and a pitch cable become notes (a poly cable, chords), and four
//! control cables become CCs. MIDI clock is the port's, set in the Rig,
//! not the module's (see [`midi_clock`](crate::link::midi_clock)): a patch
//! saved when MIDI Out had **Send Clock** turns its port's clock on as it
//! opens.
//!
//! The module only turns cables into messages, each stamped with the sample
//! it belongs on, and writes them to a ring. The link's sender thread
//! (`link::midi_sender`) sends them to the port's device as that sample is
//! heard; in the browser, a timer runs the same sender (`link::web_sender`).
//! In the offline renderer a MIDI Out runs but sends nothing.

use rtrb::Producer;
use web_time::Instant;

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Transition},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer, MAX_CHANNELS},
    SignalType,
};
use crate::link::midi::{Message, MidiBus, Outgoing, MIDI, PORTS, PORT_LABELS};

/// CC lanes on the module.
pub const CC_LANES: usize = 4;

/// The CC each lane sends unless set otherwise: mod wheel, cutoff,
/// resonance and pan, which most synths answer to.
const CC_DEFAULTS: [f32; CC_LANES] = [1.0, 74.0, 71.0, 10.0];

const CHANNEL_LABELS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// The module's ID, as patches name it.
pub const ID: &str = "output.midi";

/// Turns cables into MIDI for a device.
///
/// # Ports
///
/// **Inputs:**
/// - **Pitch** (Control, poly): V/Oct, 0.0 = C4 (note 60), read as each
///   gate rises.
/// - **Gate** (Gate, poly): a note per channel while high.
/// - **Velocity** (Control, poly): 0 to 1, read as each gate rises.
/// - **CC 1** … **CC 4** (Control): 0 to 1 sent as 0 to 127, whenever it
///   moves.
///
/// # Parameters
///
/// - **Port**: A to H; this computer's settings (the Rig panel) say which device each plays.
/// - **Channel**: the MIDI channel notes and CCs go out on.
/// - **CC 1 #** … **CC 4 #**: the controller number each lane sends.
pub struct MidiOut {
    ring: Option<Producer<Outgoing>>,
    /// Samples run so far: the clock the ring's messages are stamped on.
    sample: u64,
    /// The channel held notes were sent on.
    channel: u8,
    /// Each voice's gate at the end of the last block.
    gates: [Edge; MAX_CHANNELS],
    /// The note each voice is holding.
    voice_note: [Option<u8>; MAX_CHANNELS],
    /// How many voices hold each note: it's let go when the last does.
    held: [u8; 128],
    /// Each lane's CC number and value as last sent.
    cc_sent: [Option<(u8, u8)>; CC_LANES],
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// For the face.
    beat: f32,
    notes_sent: u32,
    dropped: u32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl MidiOut {
    /// A MIDI Out sending through the app's bus.
    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A MIDI Out writing to a ring from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        const CC_IDS: [&str; CC_LANES] = ["cc1", "cc2", "cc3", "cc4"];
        const CC_NAMES: [&str; CC_LANES] = ["CC 1", "CC 2", "CC 3", "CC 4"];
        const NUMBER_IDS: [&str; CC_LANES] = ["cc1_number", "cc2_number", "cc3_number", "cc4_number"];
        const NUMBER_NAMES: [&str; CC_LANES] = ["CC 1 #", "CC 2 #", "CC 3 #", "CC 4 #"];
        let mut ports = vec![
            PortDefinition::input_with_default("pitch", "Pitch", SignalType::Control, 0.0)
                .describe("V/Oct, 0 is C4 (note 60). A poly cable plays a chord, a note per channel"),
            PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0)
                .describe("A note starts as each channel's gate rises and ends as it falls"),
            PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 0.8)
                .describe("How hard each note is struck, 0 to 1, read as its gate rises"),
        ];
        ports.extend(CC_IDS.iter().zip(CC_NAMES).map(|(&id, name)| {
            PortDefinition::input_with_default(id, name, SignalType::Control, 0.0)
                .describe("0 to 1, sent as the CC number on the face (0 to 127) whenever it moves")
        }));
        let mut parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                .describe("Which device plays: click the device on the face, or open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The MIDI channel notes and CCs go out on"),
        ];
        parameters.extend(NUMBER_IDS.iter().zip(NUMBER_NAMES).zip(CC_DEFAULTS).map(|((&id, name), default)| {
            ParameterDefinition::new(id, name, 0.0, 127.0, default, ParameterDisplay::stepped("")).describe("The controller number this lane sends")
        }));
        Self {
            ring: bus.open_stream(),
            sample: 0,
            channel: 0,
            gates: [Edge::LOW; MAX_CHANNELS],
            voice_note: [None; MAX_CHANNELS],
            held: [0; 128],
            cc_sent: [None; CC_LANES],
            was_playing: false,
            beat: 0.0,
            notes_sent: 0,
            dropped: 0,
            ports,
            parameters,
        }
    }

    pub const PORT_PITCH: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VELOCITY: usize = 2;
    /// The first CC lane's input.
    pub const PORT_CC: usize = 3;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    /// The first CC lane's number.
    pub const PARAM_CC_NUMBER: usize = 2;

    // Readout slots

    /// The notes held, 24 to a slot, from this slot on (six slots).
    pub const READOUT_NOTES: usize = 0;
    /// Each lane's CC value as sent (0 to 127), or -1 while unpatched.
    pub const READOUT_CC: usize = 6;
    /// 1 while the patch's Clock plays.
    pub const READOUT_PLAYING: usize = 10;
    /// The beat in the bar, from 0.
    pub const READOUT_BEAT: usize = 11;
    /// Notes sent so far, wrapping at [`WRAP`](Self::WRAP).
    pub const READOUT_NOTES_SENT: usize = 12;
    /// Messages the ring had no room for, wrapping at [`WRAP`](Self::WRAP).
    pub const READOUT_DROPPED: usize = 13;
    /// 1 when there's a sender to play to (the desktop app).
    pub const READOUT_LIVE: usize = 14;

    /// Counts in the readout wrap here, well inside an f32's whole numbers.
    pub const WRAP: u32 = 4096;

    /// Whether note `note` is lit in a readout.
    pub fn note_in(readout: &Readout, note: u8) -> bool {
        let bits = readout.values[Self::READOUT_NOTES + note as usize / 24] as u32;
        bits & (1 << (note % 24)) != 0
    }

    /// Puts `message` in the ring at `offset` samples into this block.
    fn emit(&mut self, offset: usize, message: Message) {
        let Some(ring) = &mut self.ring else { return };
        if ring.push(Outgoing::Message { sample: self.sample + offset as u64, message }).is_err() {
            self.dropped = (self.dropped + 1) % Self::WRAP;
        }
    }

    fn note_on(&mut self, voice: usize, note: u8, velocity: u8, offset: usize) {
        self.note_off(voice, offset);
        let channel = self.channel;
        self.emit(offset, Message::note_on(channel, note, velocity));
        self.held[note as usize] = self.held[note as usize].saturating_add(1);
        self.voice_note[voice] = Some(note);
        self.notes_sent = (self.notes_sent + 1) % Self::WRAP;
    }

    fn note_off(&mut self, voice: usize, offset: usize) {
        let Some(note) = self.voice_note[voice].take() else { return };
        let count = &mut self.held[note as usize];
        *count = count.saturating_sub(1);
        if *count == 0 {
            let channel = self.channel;
            self.emit(offset, Message::note_off(channel, note));
        }
    }

    /// Lets go of every note held. Gates still high stay quiet until they
    /// rise again.
    fn release_all(&mut self, offset: usize) {
        for note in 0..128u8 {
            if std::mem::take(&mut self.held[note as usize]) > 0 {
                let channel = self.channel;
                self.emit(offset, Message::note_off(channel, note));
            }
        }
        self.voice_note = [None; MAX_CHANNELS];
    }
}

impl Default for MidiOut {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for MidiOut {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "MIDI Out",
            category: ModuleCategory::Output,
            description: "Play hardware and other apps: gates and pitch as notes, and four CCs. Clock goes out from the port, in the Rig",
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
        let port = (param(Self::PARAM_PORT).round().max(0.0) as usize).min(PORTS - 1);
        let channel = (param(Self::PARAM_CHANNEL).round().clamp(0.0, 15.0)) as u8;
        if let Some(ring) = &mut self.ring {
            // REAL-TIME SAFE: reading the clock and pushing to a ring. A block
            // not played as it's made (a capture) goes out as it's made
            let at = context.sent_at.unwrap_or_else(Instant::now);
            let block = Outgoing::Block { sample: self.sample, at, rate: context.sample_rate, port: port as u8 };
            if ring.push(block).is_err() {
                self.dropped = (self.dropped + 1) % Self::WRAP;
            }
        }
        if channel != self.channel {
            // Notes end on the channel they started on
            self.release_all(0);
            self.channel = channel;
        }

        let transport = &context.transport;
        let playing = transport.beat_position.is_some() && transport.playing;
        if self.was_playing && !playing {
            // Stopped mid-note: nothing will let go of it
            self.release_all(0);
        }
        self.was_playing = playing;
        let beats_per_bar = transport.time_sig_numerator.max(1) as f64;
        self.beat = transport.beat_at(frames, context.sample_rate).map_or(0.0, |b| b.rem_euclid(beats_per_bar) as f32);

        // Notes: a voice per channel of the gate (or pitch) cable
        let gate = connected_input(inputs, Self::PORT_GATE);
        let pitch = connected_input(inputs, Self::PORT_PITCH);
        let velocity = connected_input(inputs, Self::PORT_VELOCITY);
        let voices = gate.map_or(0, |g| g.channels().max(pitch.map_or(1, |p| p.channels())));
        for voice in voices..MAX_CHANNELS {
            // A voice the cable no longer carries
            self.gates[voice].reset();
            self.note_off(voice, 0);
        }
        if let Some(gate) = gate {
            // Sample by sample across the voices, so messages go in time
            // order and a note two voices share ends with the later
            for offset in 0..frames {
                for voice in 0..voices {
                    match self.gates[voice].tick(gate.voice(voice).at(offset)) {
                        None => continue,
                        Some(Transition::Falling) => {
                            self.note_off(voice, offset);
                            continue;
                        }
                        Some(Transition::Rising) => {}
                    }
                    let read = |input: Option<&SignalBuffer>, default: f32| input.map_or(default, |i| i.voice(voice).at(offset));
                    let note = (60.0 + read(pitch, 0.0) * 12.0).round().clamp(0.0, 127.0) as u8;
                    let strike = (read(velocity, 0.8) * 127.0).round().clamp(1.0, 127.0) as u8;
                    self.note_on(voice, note, strike, offset);
                }
            }
        }

        // CCs: the newest value, when it or its number has changed
        for lane in 0..CC_LANES {
            let Some(input) = connected_input(inputs, Self::PORT_CC + lane) else {
                self.cc_sent[lane] = None;
                continue;
            };
            let number = param(Self::PARAM_CC_NUMBER + lane).round().clamp(0.0, 127.0) as u8;
            let last = input.samples[..frames.min(input.samples.len())].last().copied().unwrap_or(0.0);
            let value = (last.clamp(0.0, 1.0) * 127.0).round() as u8;
            if self.cc_sent[lane] != Some((number, value)) {
                self.cc_sent[lane] = Some((number, value));
                let channel = self.channel;
                self.emit(frames.saturating_sub(1), Message::control(channel, number, value));
            }
        }

        self.sample += frames as u64;
    }

    fn reset(&mut self) {
        self.release_all(0);
        self.gates = [Edge::LOW; MAX_CHANNELS];
        self.cc_sent = [None; CC_LANES];
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        for (note, &count) in self.held.iter().enumerate() {
            if count > 0 {
                let slot = &mut values[Self::READOUT_NOTES + note / 24];
                *slot = (*slot as u32 | 1 << (note % 24)) as f32;
            }
        }
        for lane in 0..CC_LANES {
            values[Self::READOUT_CC + lane] = self.cc_sent[lane].map_or(-1.0, |(_, value)| value as f32);
        }
        values[Self::READOUT_PLAYING] = if self.was_playing { 1.0 } else { 0.0 };
        values[Self::READOUT_BEAT] = self.beat;
        values[Self::READOUT_NOTES_SENT] = self.notes_sent as f32;
        values[Self::READOUT_DROPPED] = self.dropped as f32;
        values[Self::READOUT_LIVE] = if self.ring.is_some() { 1.0 } else { 0.0 };
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use rtrb::Consumer;

    const BLOCK: usize = 64;
    const RATE: f32 = 48_000.0;

    /// A MIDI Out on a bus of its own, and the far end of its ring.
    fn module() -> (MidiOut, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let module = MidiOut::on_bus(bus);
        let ring = bus.take_new_streams().pop().unwrap();
        (module, ring)
    }

    fn defaults(module: &MidiOut) -> Vec<f32> {
        module.parameters().iter().map(|p| p.default).collect()
    }

    fn inputs(module: &MidiOut) -> Vec<SignalBuffer> {
        module.ports().iter().map(|port| SignalBuffer::unconnected(BLOCK, port.signal_type)).collect()
    }

    /// A connected poly cable, channel `c` sample `i` from `f(c, i)`.
    fn cable(channels: usize, signal_type: SignalType, f: impl Fn(usize, usize) -> f32) -> SignalBuffer {
        let mut buffer = SignalBuffer::polyphonic(BLOCK, signal_type);
        buffer.set_channels(channels);
        for c in 0..channels {
            for (i, sample) in buffer.channel_mut(c).iter_mut().enumerate() {
                *sample = f(c, i);
            }
        }
        buffer
    }

    fn run(module: &mut MidiOut, inputs: &[SignalBuffer], params: &[f32], transport: TransportState) {
        let refs: Vec<&SignalBuffer> = inputs.iter().collect();
        let mut context = ProcessContext::new(RATE, BLOCK);
        context.transport = transport;
        module.process(&refs, &mut [], params, &context);
    }

    /// The messages waiting, as (sample, bytes).
    fn messages(ring: &mut Consumer<Outgoing>) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(event) = ring.pop() {
            if let Outgoing::Message { sample, message } = event {
                out.push((sample, message.bytes().to_vec()));
            }
        }
        out
    }

    #[test]
    fn ports_and_info() {
        let (module, _) = module();
        assert_eq!(module.info().id, "output.midi");
        assert!(module.ports().iter().all(|p| p.is_input()));
        assert_eq!(module.ports()[MidiOut::PORT_CC + 3].id, "cc4");
        assert!(module.polyphonic());
        assert!(MidiOut::on_bus(Box::leak(Box::new(MidiBus::new()))).ring.is_none(), "no sender, no ring");
    }

    #[test]
    fn a_gate_plays_its_pitch() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        // D4 (two semitones up), struck at sample 10, released at 40
        inputs[MidiOut::PORT_PITCH] = cable(1, SignalType::Control, |_, _| 2.0 / 12.0);
        inputs[MidiOut::PORT_GATE] = cable(1, SignalType::Gate, |_, i| if (10..40).contains(&i) { 1.0 } else { 0.0 });
        inputs[MidiOut::PORT_VELOCITY] = cable(1, SignalType::Control, |_, _| 1.0);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(messages(&mut ring), vec![(10, vec![0x90, 62, 127]), (40, vec![0x80, 62, 0])]);
        // The next block is stamped after this one
        inputs[MidiOut::PORT_GATE] = cable(1, SignalType::Gate, |_, i| if i == 5 { 1.0 } else { 0.0 });
        run(&mut module, &inputs, &params, TransportState::new());
        let next = messages(&mut ring);
        assert_eq!(next[0].0, BLOCK as u64 + 5);
    }

    #[test]
    fn a_block_says_when_it_ran() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_PORT] = 2.0;
        let inputs = inputs(&module);
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(matches!(ring.pop(), Ok(Outgoing::Block { sample: 0, port: 2, rate: RATE, .. })));
    }

    #[test]
    fn a_poly_cable_plays_a_chord_and_stopping_lets_it_go() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        // C major, held, on channel 3
        let mut params = params;
        params[MidiOut::PARAM_CHANNEL] = 2.0;
        let chord = [0.0, 4.0, 7.0];
        inputs[MidiOut::PORT_PITCH] = cable(3, SignalType::Control, |c, _| chord[c] / 12.0);
        inputs[MidiOut::PORT_GATE] = cable(3, SignalType::Gate, |_, _| 1.0);
        let playing = TransportState { beat_position: Some(0.0), ..TransportState::playing_at(120.0) };
        run(&mut module, &inputs, &params, playing);
        let ons: Vec<Vec<u8>> = messages(&mut ring).into_iter().map(|(_, m)| m).collect();
        assert_eq!(ons, [vec![0x92, 60, 102], vec![0x92, 64, 102], vec![0x92, 67, 102]]);
        let readout = module.readout(&params).unwrap();
        assert!(MidiOut::note_in(&readout, 64) && !MidiOut::note_in(&readout, 65));
        // Stopped mid-chord, gates still high: every note ends, and none restarts
        let stopped = TransportState { playing: false, beat_position: Some(0.1), ..TransportState::playing_at(120.0) };
        run(&mut module, &inputs, &params, stopped);
        let offs: Vec<Vec<u8>> = messages(&mut ring).into_iter().map(|(_, m)| m).collect();
        assert_eq!(offs, [vec![0x82, 60, 0], vec![0x82, 64, 0], vec![0x82, 67, 0]]);
        run(&mut module, &inputs, &params, stopped);
        assert!(messages(&mut ring).is_empty());
    }

    #[test]
    fn a_note_two_voices_share_ends_with_the_last() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        inputs[MidiOut::PORT_GATE] = cable(2, SignalType::Gate, |c, i| if i < 20 + c * 20 { 1.0 } else { 0.0 });
        run(&mut module, &inputs, &params, TransportState::new());
        let sent = messages(&mut ring);
        assert_eq!(sent.iter().filter(|(_, m)| m[0] == 0x90).count(), 2);
        assert_eq!(sent.iter().filter(|(_, m)| m[0] == 0x80).map(|(s, _)| *s).collect::<Vec<_>>(), [40]);
    }

    #[test]
    fn ccs_go_out_when_they_move() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        inputs[MidiOut::PORT_CC + 1] = cable(1, SignalType::Control, |_, _| 0.5);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(messages(&mut ring), vec![((BLOCK - 1) as u64, vec![0xB0, 74, 64])]);
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "unchanged");
        inputs[MidiOut::PORT_CC + 1] = cable(1, SignalType::Control, |_, _| 2.0);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(messages(&mut ring)[0].1, vec![0xB0, 74, 127], "clamped");
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_CC + 1], 127.0);
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_CC], -1.0);
    }

    #[test]
    fn a_playing_clock_sends_no_clock_from_the_module() {
        // The port sends it, once, however many MIDI Outs play it (#437)
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let inputs = inputs(&module);
        let mut beat = 0.0;
        for _ in 0..200 {
            let transport = TransportState { beat_position: Some(beat), ..TransportState::playing_at(120.0) };
            run(&mut module, &inputs, &params, transport);
            beat += BLOCK as f64 * 2.0 / RATE as f64;
        }
        assert!(messages(&mut ring).is_empty());
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_PLAYING], 1.0);
    }

    #[test]
    fn midi_out_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiOut>();
    }
}
