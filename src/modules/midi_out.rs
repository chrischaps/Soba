//! MIDI Out module: plays hardware and other apps from the patch.
//!
//! A gate and a pitch cable become notes (a poly cable, chords), and four
//! control cables become CCs. With **Send Clock** on, the patch's Clock
//! goes out as MIDI clock, 24 ticks a beat with Start, Stop and Continue,
//! so a drum machine or a DAW follows Soba's tempo.
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
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer, MAX_CHANNELS},
    SignalType,
};
use crate::link::midi::{status, Message, MidiBus, Outgoing, MIDI, PORTS, PORT_LABELS};

/// CC lanes on the module.
pub const CC_LANES: usize = 4;

/// The CC each lane sends unless set otherwise: mod wheel, cutoff,
/// resonance and pan, which most synths answer to.
const CC_DEFAULTS: [f32; CC_LANES] = [1.0, 74.0, 71.0, 10.0];

/// MIDI clock ticks in a beat.
const PPQN: f64 = 24.0;

/// A gate above this is high.
const GATE_THRESHOLD: f32 = 0.5;

const CHANNEL_LABELS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

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
/// - **Port**: A to D; the settings say which device each plays.
/// - **Channel**: the MIDI channel notes and CCs go out on.
/// - **CC 1 #** … **CC 4 #**: the controller number each lane sends.
/// - **Send Clock**: sends the patch's Clock as MIDI clock.
pub struct MidiOut {
    ring: Option<Producer<Outgoing>>,
    /// Samples run so far: the clock the ring's messages are stamped on.
    sample: u64,
    /// The channel held notes were sent on.
    channel: u8,
    /// Each voice's gate at the end of the last block.
    gate_high: [bool; MAX_CHANNELS],
    /// The note each voice is holding.
    voice_note: [Option<u8>; MAX_CHANNELS],
    /// How many voices hold each note: it's let go when the last does.
    held: [u8; 128],
    /// Each lane's CC number and value as last sent.
    cc_sent: [Option<(u8, u8)>; CC_LANES],
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// Whether clock is going out (Start or Continue sent, Stop not yet).
    clock_running: bool,
    /// The next clock tick to send, counting 24 a beat from beat 0.
    next_tick: i64,
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
                .describe("Which device plays: the face, or Settings > MIDI Out, gives each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The MIDI channel notes and CCs go out on"),
        ];
        parameters.extend(NUMBER_IDS.iter().zip(NUMBER_NAMES).zip(CC_DEFAULTS).map(|((&id, name), default)| {
            ParameterDefinition::new(id, name, 0.0, 127.0, default, ParameterDisplay::stepped("")).describe("The controller number this lane sends")
        }));
        parameters.push(
            ParameterDefinition::toggle("send_clock", "Send Clock", false)
                .describe("Sends the patch's Clock as MIDI clock (24 a beat) with Start, Stop and Continue, for other gear to follow"),
        );
        Self {
            ring: bus.open_stream(),
            sample: 0,
            channel: 0,
            gate_high: [false; MAX_CHANNELS],
            voice_note: [None; MAX_CHANNELS],
            held: [0; 128],
            cc_sent: [None; CC_LANES],
            was_playing: false,
            clock_running: false,
            next_tick: 0,
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
    pub const PARAM_SEND_CLOCK: usize = 2 + CC_LANES;

    // Readout slots

    /// The notes held, 24 to a slot, from this slot on (six slots).
    pub const READOUT_NOTES: usize = 0;
    /// Each lane's CC value as sent (0 to 127), or -1 while unpatched.
    pub const READOUT_CC: usize = 6;
    /// 0 with Send Clock off, 1 while on and waiting, 2 while clock runs.
    pub const READOUT_CLOCK: usize = 10;
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

    fn stop_clock(&mut self, offset: usize) {
        if std::mem::take(&mut self.clock_running) {
            self.emit(offset, Message::real_time(status::STOP));
        }
    }

    /// Sends this block's MIDI clock.
    fn run_clock(&mut self, send: bool, context: &ProcessContext) {
        let transport = &context.transport;
        let frames = context.block_size;
        let Some(start) = transport.beat_position.filter(|_| send && transport.playing) else {
            self.stop_clock(0);
            return;
        };
        let beats_per_sample = transport.tempo_bpm.unwrap_or(120.0) as f64 / 60.0 / context.sample_rate as f64;
        let end = start + frames as f64 * beats_per_sample;
        // The first sample at or past `beat` (a hair's leeway for rounding)
        let at = |beat: f64| (((beat - start) / beats_per_sample - 1e-6).ceil().max(0.0) as usize).min(frames.saturating_sub(1));
        let tick = 1.0 / PPQN;

        // Moved more than a tick from where it was going (a jump in the
        // song): stop, and start again from the new place
        if self.clock_running && (start < (self.next_tick - 1) as f64 * tick - tick || start > self.next_tick as f64 * tick + tick) {
            self.stop_clock(0);
        }
        if !self.clock_running {
            if end <= 0.0 {
                return; // a count-in
            }
            if start <= 1e-9 {
                // From the top: Start, and the first tick is beat 0
                self.emit(at(0.0), Message::real_time(status::START));
                self.next_tick = 0;
            } else {
                // Partway in: where to carry on from, then Continue. The
                // follower waits for the next sixteenth's tick
                let sixteenth = (start * 4.0).ceil();
                self.emit(0, Message::song_position(sixteenth.min(16383.0) as u16));
                self.emit(0, Message::real_time(status::CONTINUE));
                self.next_tick = sixteenth as i64 * 6;
            }
            self.clock_running = true;
        }
        loop {
            // A tick a rounding error short of the block's end is the next block's first
            let offset = ((self.next_tick as f64 * tick - start) / beats_per_sample - 1e-6).ceil().max(0.0) as usize;
            if offset >= frames {
                break;
            }
            self.emit(offset, Message::real_time(status::CLOCK));
            self.next_tick += 1;
        }
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
            id: "output.midi",
            name: "MIDI Out",
            category: ModuleCategory::Output,
            description: "Play hardware and other apps: gates and pitch as notes, four CCs, and the Clock as MIDI clock",
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

        // The clock first, so a Start lands before the notes on its beat
        self.run_clock(param(Self::PARAM_SEND_CLOCK) > 0.5, context);
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
            self.gate_high[voice] = false;
            self.note_off(voice, 0);
        }
        if let Some(gate) = gate {
            // Sample by sample across the voices, so messages go in time
            // order and a note two voices share ends with the later
            for offset in 0..frames {
                for voice in 0..voices {
                    let high = gate.voice(voice).samples.get(offset).is_some_and(|&s| s > GATE_THRESHOLD);
                    if high == self.gate_high[voice] {
                        continue;
                    }
                    self.gate_high[voice] = high;
                    if !high {
                        self.note_off(voice, offset);
                        continue;
                    }
                    let read = |input: Option<&SignalBuffer>, default: f32| {
                        input.and_then(|i| i.voice(voice).samples.get(offset).copied()).unwrap_or(default)
                    };
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
        self.stop_clock(0);
        self.gate_high = [false; MAX_CHANNELS];
        self.cc_sent = [None; CC_LANES];
    }

    fn readout(&self, params: &[f32]) -> Option<Readout> {
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
        let send = params.get(Self::PARAM_SEND_CLOCK).is_some_and(|&p| p > 0.5);
        values[Self::READOUT_CLOCK] = match (send, self.clock_running) {
            (false, _) => 0.0,
            (true, false) => 1.0,
            (true, true) => 2.0,
        };
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
        assert_eq!(module.parameters()[MidiOut::PARAM_SEND_CLOCK].id, "send_clock");
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

    /// Runs `blocks` blocks of the Clock from `beat` at `bpm`, as the
    /// engine would, the beat advancing with each. Returns what was sent.
    #[allow(clippy::too_many_arguments)]
    fn clock(
        module: &mut MidiOut,
        ring: &mut Consumer<Outgoing>,
        params: &[f32],
        beat: &mut f64,
        bpm: f32,
        blocks: usize,
        playing: bool,
    ) -> Vec<(u64, Vec<u8>)> {
        let inputs = inputs(module);
        let mut sent = Vec::new();
        for _ in 0..blocks {
            let transport = TransportState { playing, beat_position: Some(*beat), ..TransportState::playing_at(bpm) };
            run(module, &inputs, params, transport);
            sent.extend(messages(ring));
            if playing {
                *beat += BLOCK as f64 * bpm as f64 / 60.0 / RATE as f64;
            }
        }
        sent
    }

    #[test]
    fn the_clock_ticks_24_a_beat_from_start() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_SEND_CLOCK] = 1.0;
        let mut beat = 0.0;
        // Just short of four beats at 120 BPM (2 s is 1500 blocks)
        let sent = clock(&mut module, &mut ring, &params, &mut beat, 120.0, 1499, true);
        assert_eq!(sent[0], (0, vec![status::START]));
        let ticks: Vec<u64> = sent.iter().filter(|(_, m)| m == &[status::CLOCK]).map(|(s, _)| *s).collect();
        assert_eq!(ticks.len(), 96);
        // Every 1000 samples: 24 a beat at 2 beats a second, at 48 kHz
        assert!(ticks.iter().enumerate().all(|(k, &s)| s == k as u64 * 1000), "{:?}", &ticks[..4]);
        // Stopped, then played on from beat 4: Stop, then the position and Continue
        let stopped = clock(&mut module, &mut ring, &params, &mut beat, 120.0, 2, false);
        assert_eq!(stopped.iter().map(|(_, m)| m[0]).collect::<Vec<_>>(), [status::STOP]);
        beat = 4.1;
        let resumed = clock(&mut module, &mut ring, &params, &mut beat, 120.0, 1, true);
        assert_eq!(resumed[0].1, Message::song_position(17).bytes(), "carries on from the next sixteenth");
        assert_eq!(resumed[1].1, vec![status::CONTINUE]);
    }

    #[test]
    fn a_count_in_starts_the_clock_on_the_downbeat() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_SEND_CLOCK] = 1.0;
        // Half a beat before the top
        let mut beat = -0.5;
        let sent = clock(&mut module, &mut ring, &params, &mut beat, 120.0, 200, true);
        let start = sent.iter().position(|(_, m)| m == &[status::START]).unwrap();
        assert_eq!(sent[start].0, 12_000, "half a beat at 120 BPM");
        assert_eq!(sent[start + 1], (12_000, vec![status::CLOCK]));
    }

    #[test]
    fn a_jump_in_the_song_restarts_the_clock() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_SEND_CLOCK] = 1.0;
        let mut beat = 0.0;
        clock(&mut module, &mut ring, &params, &mut beat, 120.0, 100, true);
        beat = 0.0;
        let sent: Vec<u8> = clock(&mut module, &mut ring, &params, &mut beat, 120.0, 1, true).iter().map(|(_, m)| m[0]).collect();
        assert_eq!(&sent[..3], [status::STOP, status::START, status::CLOCK]);
    }

    #[test]
    fn clock_off_sends_none() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut beat = 0.0;
        assert!(clock(&mut module, &mut ring, &params, &mut beat, 120.0, 100, true).is_empty());
    }

    #[test]
    fn midi_out_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MidiOut>();
    }
}
