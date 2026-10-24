//! MIDI Out module: plays hardware and other apps from the patch.
//!
//! A gate and a pitch cable become notes (a poly cable, chords); a bend
//! and a pressure cable become Pitch Bend and aftertouch; four control
//! cables become CCs, each 7-bit, 14-bit or an NRPN; and a Program, with
//! its bank, picks the device's sound (#439). MIDI clock is the port's,
//! set in the Rig, not the module's (see
//! [`midi_clock`](crate::link::midi_clock)): a patch saved when MIDI Out
//! had **Send Clock** turns its port's clock on as it opens.
//!
//! The module only turns cables into messages, each stamped with the sample
//! it belongs on, and writes them to a ring through a
//! [`MidiWriter`]. The link's sender thread (`link::midi_sender`) sends
//! them to the port's device as that sample is heard, within the port's
//! budget; in the browser, a timer runs the same sender
//! (`link::web_sender`). In the offline renderer a MIDI Out runs but sends
//! nothing.
//!
//! Controllers go where they change within a block, at most once every
//! [`LANE_GAP`](crate::link::midi_writer::LANE_GAP) a lane, not once a
//! block: an LFO into a CC sweeps as smoothly as the device can take.

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Transition},
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::{ParameterDefinition, ParameterDisplay},
    port::PortDefinition,
    signal::{connected_input, SignalBuffer, MAX_CHANNELS},
    SignalType,
};
use crate::link::midi::{CcMode, Control, Message, MidiBus, Program, BEND_CENTER, MAX_14, MIDI, PORTS, PORT_LABELS, RPN_BEND_RANGE};
use crate::link::midi_writer::{lane_gap, ControlLane, MidiWriter, ProgramSlot, WRAP};

/// CC lanes on the module.
pub const CC_LANES: usize = 4;

/// The CC each lane sends unless set otherwise: mod wheel, cutoff,
/// resonance and pan, which most synths answer to.
const CC_DEFAULTS: [f32; CC_LANES] = [1.0, 74.0, 71.0, 10.0];

const CHANNEL_LABELS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// The widest bend range a device is told, in semitones: two octaves.
pub const MAX_BEND_RANGE: f32 = 24.0;

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
/// - **Bend** (Control): -1 to 1, sent as Pitch Bend where it moves.
/// - **Pressure** (Control, poly): 0 to 1, sent as Channel Pressure, or as
///   each note's Poly Pressure from a poly cable.
/// - **CC 1** … **CC 4** (Control): 0 to 1, sent where it moves, as the
///   lane's mode says.
/// - **Send Program** (Gate): sends the Bank and Program again as it rises.
///
/// # Parameters
///
/// - **Port**: A to H; this computer's settings (the Rig panel) say which device each plays.
/// - **Channel**: the MIDI channel everything goes out on.
/// - **CC 1 #** … **CC 4 #**: the controller number each lane sends.
/// - **Bend Range**: the bend range to set on the device (RPN 0), or Off.
/// - **Program**, **Bank MSB**, **Bank LSB**: the sound to pick, or Off.
/// - **CC Mode 1** … **4**, **NRPN 1** … **4**: how each lane sends, set
///   from its menu on the face.
pub struct MidiOut {
    out: MidiWriter,
    /// The port and channel it played last block.
    port: usize,
    channel: u8,
    /// Each voice's gate at the end of the last block.
    gates: [Edge; MAX_CHANNELS],
    /// The note each voice is holding.
    voice_note: [Option<u8>; MAX_CHANNELS],
    /// Where each voice's note began in this block: 0 for one held from before.
    voice_from: [usize; MAX_CHANNELS],
    /// How many voices hold each note: it's let go when the last does.
    held: [u8; 128],
    lanes: [ControlLane; CC_LANES],
    bend: ControlLane,
    pressure: ControlLane,
    /// Each voice's Poly Pressure, from a poly Pressure cable.
    poly_pressure: [ControlLane; MAX_CHANNELS],
    program: ProgramSlot,
    /// The bend range the device was told, and where: (port, channel, semitones).
    bend_range: Option<(usize, u8, u8)>,
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// For the face.
    beat: f32,
    notes_sent: u32,
    programs_sent: u32,
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
        const MODE_IDS: [&str; CC_LANES] = ["cc1_mode", "cc2_mode", "cc3_mode", "cc4_mode"];
        const MODE_NAMES: [&str; CC_LANES] = ["CC Mode 1", "CC Mode 2", "CC Mode 3", "CC Mode 4"];
        const NRPN_IDS: [&str; CC_LANES] = ["cc1_nrpn", "cc2_nrpn", "cc3_nrpn", "cc4_nrpn"];
        const NRPN_NAMES: [&str; CC_LANES] = ["NRPN 1", "NRPN 2", "NRPN 3", "NRPN 4"];
        let mut ports = vec![
            PortDefinition::input_with_default("pitch", "Pitch", SignalType::Control, 0.0)
                .describe("V/Oct, 0 is C4 (note 60). A poly cable plays a chord, a note per channel"),
            PortDefinition::input_with_default("gate", "Gate", SignalType::Gate, 0.0)
                .describe("A note starts as each channel's gate rises and ends as it falls"),
            PortDefinition::input_with_default("velocity", "Velocity", SignalType::Control, 0.8)
                .describe("How hard each note is struck, 0 to 1, read as its gate rises"),
            PortDefinition::input_with_default("bend", "Bend", SignalType::Control, 0.0)
                .describe("-1 to 1, full throw down to full throw up, sent as Pitch Bend where it moves. Unplugged, it springs back to the middle"),
            PortDefinition::input_with_default("pressure", "Pressure", SignalType::Control, 0.0)
                .describe("0 to 1, sent as aftertouch: Channel Pressure from a one-channel cable, each note's own Poly Pressure from a poly cable"),
        ];
        ports.extend(CC_IDS.iter().zip(CC_NAMES).map(|(&id, name)| {
            PortDefinition::input_with_default(id, name, SignalType::Control, 0.0)
                .describe("0 to 1, sent on the lane's CC number (or NRPN) wherever it moves. Right-click its bar on the face for 14-bit or NRPN")
        }));
        ports.push(
            PortDefinition::input_with_default("send_program", "Send Program", SignalType::Gate, 0.0)
                .describe("Sends the Bank and Program again as it rises: at the top of a song, say"),
        );
        let mut parameters = vec![
            ParameterDefinition::choice("port", "Port", PORT_LABELS, 0)
                .describe("Which device plays: click the device on the face, or open the Rig (Ctrl+Shift+H) to give each port one"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0).describe("The MIDI channel everything goes out on"),
        ];
        parameters.extend(NUMBER_IDS.iter().zip(NUMBER_NAMES).zip(CC_DEFAULTS).map(|((&id, name), default)| {
            ParameterDefinition::new(id, name, 0.0, 127.0, default, ParameterDisplay::stepped("")).describe("The controller number this lane sends")
        }));
        parameters.extend([
            ParameterDefinition::new("bend_range", "Bend Range", 0.0, MAX_BEND_RANGE, 0.0, ParameterDisplay::stepped("st or off"))
                .describe("How far a full bend goes, in semitones: Soba tells the device (RPN 0, Pitch Bend Sensitivity). Off leaves the device's own"),
            ParameterDefinition::new("program", "Program", -1.0, 127.0, -1.0, ParameterDisplay::stepped("or off"))
                .describe("The Program Change to send, 0 to 127 as sent (a manual that counts from 1 calls 0 its first). Sent as it changes, and as Send Program rises. Off sends none"),
            ParameterDefinition::new("bank_msb", "Bank MSB", -1.0, 127.0, -1.0, ParameterDisplay::stepped("or off"))
                .describe("The Bank Select MSB (CC 0) sent before the Program. Off sends none"),
            ParameterDefinition::new("bank_lsb", "Bank LSB", -1.0, 127.0, -1.0, ParameterDisplay::stepped("or off"))
                .describe("The Bank Select LSB (CC 32) sent before the Program. Off sends none"),
        ]);
        parameters.extend(MODE_IDS.iter().zip(MODE_NAMES).map(|(&id, name)| {
            ParameterDefinition::choice(id, name, CcMode::LABELS, 0)
                .describe("How the lane sends: one 7-bit CC, a 14-bit pair (CC n and n + 32, for n up to 31), or an NRPN")
        }));
        parameters.extend(NRPN_IDS.iter().zip(NRPN_NAMES).map(|(&id, name)| {
            ParameterDefinition::new(id, name, 0.0, MAX_14 as f32, 0.0, ParameterDisplay::stepped(""))
                .describe("The NRPN the lane sends in NRPN mode: its MSB × 128 + its LSB")
        }));
        Self {
            out: MidiWriter::on_bus(bus),
            port: 0,
            channel: 0,
            gates: [Edge::LOW; MAX_CHANNELS],
            voice_note: [None; MAX_CHANNELS],
            voice_from: [0; MAX_CHANNELS],
            held: [0; 128],
            lanes: [ControlLane::new(); CC_LANES],
            bend: ControlLane::new(),
            pressure: ControlLane::new(),
            poly_pressure: [ControlLane::new(); MAX_CHANNELS],
            program: ProgramSlot::new(),
            bend_range: None,
            was_playing: false,
            beat: 0.0,
            notes_sent: 0,
            programs_sent: 0,
            ports,
            parameters,
        }
    }

    pub const PORT_PITCH: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VELOCITY: usize = 2;
    pub const PORT_BEND: usize = 3;
    pub const PORT_PRESSURE: usize = 4;
    /// The first CC lane's input.
    pub const PORT_CC: usize = 5;
    pub const PORT_SEND_PROGRAM: usize = Self::PORT_CC + CC_LANES;

    pub const PARAM_PORT: usize = 0;
    pub const PARAM_CHANNEL: usize = 1;
    /// The first CC lane's number.
    pub const PARAM_CC_NUMBER: usize = 2;
    pub const PARAM_BEND_RANGE: usize = Self::PARAM_CC_NUMBER + CC_LANES;
    pub const PARAM_PROGRAM: usize = Self::PARAM_BEND_RANGE + 1;
    pub const PARAM_BANK_MSB: usize = Self::PARAM_PROGRAM + 1;
    pub const PARAM_BANK_LSB: usize = Self::PARAM_BANK_MSB + 1;
    /// The first CC lane's mode (a [`CcMode`]).
    pub const PARAM_CC_MODE: usize = Self::PARAM_BANK_LSB + 1;
    /// The first CC lane's NRPN.
    pub const PARAM_NRPN: usize = Self::PARAM_CC_MODE + CC_LANES;

    // Readout slots

    /// The notes held, 24 to a slot, from this slot on (six slots).
    pub const READOUT_NOTES: usize = 0;
    /// Each lane's value as sent, 0 to 127 (finer in 14-bit), or -1 while unpatched.
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
    /// The bend as sent, -1 to 1, or below -1 while unpatched.
    pub const READOUT_BEND: usize = 15;
    /// The pressure as sent, 0 to 1 (a poly cable's highest), or -1 while unpatched.
    pub const READOUT_PRESSURE: usize = 16;
    /// 1 while the pressure goes note by note (Poly Pressure).
    pub const READOUT_POLY: usize = 17;
    /// Programs sent so far, wrapping at [`WRAP`](Self::WRAP).
    pub const READOUT_PROGRAMS_SENT: usize = 18;

    /// Counts in the readout wrap here, well inside an f32's whole numbers.
    pub const WRAP: u32 = WRAP;

    /// Whether note `note` is lit in a readout.
    pub fn note_in(readout: &Readout, note: u8) -> bool {
        let bits = readout.values[Self::READOUT_NOTES + note as usize / 24] as u32;
        bits & (1 << (note % 24)) != 0
    }

    fn note_on(&mut self, voice: usize, note: u8, velocity: u8, offset: usize) {
        self.note_off(voice, offset);
        self.out.message(offset, Message::note_on(self.channel, note, velocity));
        self.held[note as usize] = self.held[note as usize].saturating_add(1);
        self.voice_note[voice] = Some(note);
        self.voice_from[voice] = offset;
        // Its pressure goes after it, afresh
        self.poly_pressure[voice].reset();
        self.notes_sent = (self.notes_sent + 1) % Self::WRAP;
    }

    fn note_off(&mut self, voice: usize, offset: usize) {
        let Some(note) = self.voice_note[voice].take() else { return };
        let count = &mut self.held[note as usize];
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.out.message(offset, Message::note_off(self.channel, note));
        }
    }

    /// Lets go of every note held. Gates still high stay quiet until they
    /// rise again.
    fn release_all(&mut self, offset: usize) {
        for note in 0..128u8 {
            if std::mem::take(&mut self.held[note as usize]) > 0 {
                self.out.message(offset, Message::note_off(self.channel, note));
            }
        }
        self.voice_note = [None; MAX_CHANNELS];
    }

    /// Lets the bend spring back and the pressure off, on the channel
    /// they went out on, if they're away from rest.
    fn relax(&mut self, offset: usize) {
        if matches!(self.bend.sent(), Some(Control::Bend { value, .. }) if value != BEND_CENTER) {
            self.out.control(offset, Control::Bend { channel: self.channel, value: BEND_CENTER });
        }
        if matches!(self.pressure.sent(), Some(Control::Pressure { value, .. }) if value > 0) {
            self.out.control(offset, Control::Pressure { channel: self.channel, value: 0 });
        }
        self.bend.reset();
        self.pressure.reset();
        self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
    }

    /// The Program and bank the knobs choose, if one is.
    fn chosen_program(param: impl Fn(usize) -> f32) -> Option<Program> {
        let seven = |index: usize| {
            let value = param(index).round();
            (value >= 0.0).then(|| value.min(127.0) as u8)
        };
        Some(Program { bank_msb: seven(Self::PARAM_BANK_MSB), bank_lsb: seven(Self::PARAM_BANK_LSB), program: seven(Self::PARAM_PROGRAM)? })
    }

    /// What a lane last sent, 0 to 127.
    fn lane_value(control: Option<Control>) -> f32 {
        match control {
            Some(Control::Cc { value, .. }) => value as f32,
            Some(Control::Cc14 { value, .. } | Control::Nrpn { value, .. }) => value as f32 / MAX_14 as f32 * 127.0,
            _ => -1.0,
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
            id: ID,
            name: "MIDI Out",
            category: ModuleCategory::Output,
            description: "Play hardware and other apps: notes, bend, pressure, four CCs (7-bit, 14-bit or NRPN) and programs. Clock goes out from the port, in the Rig",
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
        let channel = (param(Self::PARAM_CHANNEL).round().clamp(0.0, 15.0)) as u8;
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        self.out.begin(context, port);
        if port != self.port {
            // The new device hears every controller afresh. The sender lets
            // go of the notes, and the bend, on the old one
            self.lanes = [ControlLane::new(); CC_LANES];
            self.bend.reset();
            self.pressure.reset();
            self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
            self.port = port;
        }
        if channel != self.channel {
            // Notes end, and the bend springs back, on the channel they started on
            self.release_all(0);
            self.relax(0);
            self.channel = channel;
        }
        self.voice_from = [0; MAX_CHANNELS];

        let transport = &context.transport;
        let playing = transport.beat_position.is_some() && transport.playing;
        if self.was_playing && !playing {
            // Stopped mid-note: nothing will let go of it
            self.release_all(0);
        }
        self.was_playing = playing;
        let beats_per_bar = transport.time_sig_numerator.max(1) as f64;
        self.beat = transport.beat_at(frames, context.sample_rate).map_or(0.0, |b| b.rem_euclid(beats_per_bar) as f32);

        // The sound first, so the block's notes play it: the bank and
        // program, then the bend range, which a new program may reset
        let trigger = connected_input(inputs, Self::PORT_SEND_PROGRAM).map(|t| &t.samples[..frames.min(t.samples.len())]);
        if self.program.run(&mut self.out, port, channel, Self::chosen_program(param), trigger, frames) {
            self.programs_sent = (self.programs_sent + 1) % Self::WRAP;
            self.bend_range = None;
        }
        let range = param(Self::PARAM_BEND_RANGE).round().clamp(0.0, MAX_BEND_RANGE) as u8;
        if range == 0 {
            self.bend_range = None;
        } else if self.bend_range != Some((port, channel, range)) {
            // Semitones in the coarse half, no cents
            self.out.control(0, Control::Rpn { channel, parameter: RPN_BEND_RANGE, value: (range as u16) << 7 });
            self.bend_range = Some((port, channel, range));
        }

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

        // The bend, where it moves; unplugged, it springs back
        match connected_input(inputs, Self::PORT_BEND) {
            Some(bend) => self.bend.run(&mut self.out, 0..frames, gap, |i| Control::bend(channel, bend.at(i))),
            None => {
                if matches!(self.bend.sent(), Some(Control::Bend { value, .. }) if value != BEND_CENTER) {
                    self.out.control(0, Control::Bend { channel, value: BEND_CENTER });
                }
                self.bend.reset();
            }
        }

        // The pressure: the channel's from one cable, each note's from a poly one
        match connected_input(inputs, Self::PORT_PRESSURE) {
            Some(pressure) if pressure.channels() > 1 => {
                self.pressure.reset();
                for voice in 0..MAX_CHANNELS {
                    let Some(note) = self.voice_note[voice].filter(|_| voice < pressure.channels()) else {
                        self.poly_pressure[voice].reset();
                        continue;
                    };
                    let cable = pressure.voice(voice);
                    let from = self.voice_from[voice];
                    self.poly_pressure[voice].run(&mut self.out, from..frames, gap, |i| Control::PolyPressure {
                        channel,
                        note,
                        value: Control::seven(cable.at(i)),
                    });
                }
            }
            Some(pressure) => {
                self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
                self.pressure.run(&mut self.out, 0..frames, gap, |i| Control::Pressure { channel, value: Control::seven(pressure.at(i)) });
            }
            None => {
                if matches!(self.pressure.sent(), Some(Control::Pressure { value, .. }) if value > 0) {
                    self.out.control(0, Control::Pressure { channel, value: 0 });
                }
                self.pressure.reset();
                self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
            }
        }

        // CCs: wherever they move, as each lane's mode sends them
        for lane in 0..CC_LANES {
            let Some(input) = connected_input(inputs, Self::PORT_CC + lane) else {
                self.lanes[lane].reset();
                continue;
            };
            let number = param(Self::PARAM_CC_NUMBER + lane).round().clamp(0.0, 127.0) as u8;
            let mode = CcMode::from_param(param(Self::PARAM_CC_MODE + lane));
            let nrpn = param(Self::PARAM_NRPN + lane).round().clamp(0.0, MAX_14 as f32) as u16;
            self.lanes[lane].run(&mut self.out, 0..frames, gap, |i| Control::lane(mode, channel, number, nrpn, input.at(i)));
        }

        self.out.end(frames);
    }

    fn reset(&mut self) {
        self.release_all(0);
        self.relax(0);
        self.gates = [Edge::LOW; MAX_CHANNELS];
        self.lanes = [ControlLane::new(); CC_LANES];
        self.program.reset();
        self.bend_range = None;
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
            values[Self::READOUT_CC + lane] = Self::lane_value(self.lanes[lane].sent());
        }
        values[Self::READOUT_PLAYING] = if self.was_playing { 1.0 } else { 0.0 };
        values[Self::READOUT_BEAT] = self.beat;
        values[Self::READOUT_NOTES_SENT] = self.notes_sent as f32;
        values[Self::READOUT_DROPPED] = self.out.dropped() as f32;
        values[Self::READOUT_LIVE] = if self.out.is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_BEND] = match self.bend.sent() {
            Some(Control::Bend { value, .. }) => (value as f32 - BEND_CENTER as f32) / BEND_CENTER as f32,
            _ => -2.0,
        };
        let poly = self.poly_pressure.iter().filter_map(|lane| match lane.sent() {
            Some(Control::PolyPressure { value, .. }) => Some(value),
            _ => None,
        });
        let (pressure, is_poly) = match self.pressure.sent() {
            Some(Control::Pressure { value, .. }) => (value as f32 / 127.0, false),
            _ => match poly.max() {
                Some(value) => (value as f32 / 127.0, true),
                None => (-1.0, false),
            },
        };
        values[Self::READOUT_PRESSURE] = pressure;
        values[Self::READOUT_POLY] = if is_poly { 1.0 } else { 0.0 };
        values[Self::READOUT_PROGRAMS_SENT] = self.programs_sent as f32;
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use crate::link::midi::Outgoing;
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

    /// The messages waiting, as (sample, bytes): a controller's each.
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

    #[test]
    fn ports_and_info() {
        let (module, _) = module();
        assert_eq!(module.info().id, "output.midi");
        assert!(module.ports().iter().all(|p| p.is_input()));
        assert_eq!(module.ports()[MidiOut::PORT_CC + 3].id, "cc4");
        assert_eq!(module.ports()[MidiOut::PORT_SEND_PROGRAM].id, "send_program");
        assert_eq!(module.parameters()[MidiOut::PARAM_NRPN + 3].name, "NRPN 4");
        assert_eq!(module.parameters().len(), MidiOut::PARAM_NRPN + CC_LANES);
        assert!(module.polyphonic());
        assert!(!MidiOut::on_bus(Box::leak(Box::new(MidiBus::new()))).out.is_live(), "no sender, no ring");
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
        assert_eq!(bytes(&mut ring), [vec![0x92, 60, 102], vec![0x92, 64, 102], vec![0x92, 67, 102]]);
        let readout = module.readout(&params).unwrap();
        assert!(MidiOut::note_in(&readout, 64) && !MidiOut::note_in(&readout, 65));
        // Stopped mid-chord, gates still high: every note ends, and none restarts
        let stopped = TransportState { playing: false, beat_position: Some(0.1), ..TransportState::playing_at(120.0) };
        run(&mut module, &inputs, &params, stopped);
        assert_eq!(bytes(&mut ring), [vec![0x82, 60, 0], vec![0x82, 64, 0], vec![0x82, 67, 0]]);
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
        assert_eq!(messages(&mut ring), vec![(0, vec![0xB0, 74, 64])], "at once");
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "unchanged");
        inputs[MidiOut::PORT_CC + 1] = cable(1, SignalType::Control, |_, _| 2.0);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(messages(&mut ring)[0].1, vec![0xB0, 74, 127], "clamped");
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_CC + 1], 127.0);
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_CC], -1.0);
    }

    #[test]
    fn a_cc_sweep_goes_out_where_it_moves_within_the_block() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        // A ramp across four 64-sample blocks: a new value every two samples
        let mut block = 0;
        let mut sent = Vec::new();
        while block < 4 {
            let start = block * BLOCK;
            inputs[MidiOut::PORT_CC] = cable(1, SignalType::Control, move |_, i| (start + i) as f32 / 256.0);
            run(&mut module, &inputs, &params, TransportState::new());
            sent.extend(messages(&mut ring));
            block += 1;
        }
        // A millisecond is 48 samples: no closer than that, but each block more than once at the end
        let samples: Vec<u64> = sent.iter().map(|(s, _)| *s).collect();
        assert!(samples.windows(2).all(|w| w[1] - w[0] >= 48), "{samples:?}");
        assert_eq!(samples.len(), 6, "every millisecond across 256 samples: {samples:?}");
        assert_eq!(sent[1], (48, vec![0xB0, 1, (48.0f32 / 256.0 * 127.0).round() as u8]));
    }

    #[test]
    fn lanes_send_14_bit_pairs_and_nrpns() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_CC_NUMBER] = 7.0;
        params[MidiOut::PARAM_CC_MODE] = CcMode::Bits14 as u8 as f32;
        params[MidiOut::PARAM_CC_MODE + 1] = CcMode::Nrpn as u8 as f32;
        // NRPN 1:20, as a manual writes it, is 148
        params[MidiOut::PARAM_NRPN + 1] = 148.0;
        let mut inputs = inputs(&module);
        inputs[MidiOut::PORT_CC] = cable(1, SignalType::Control, |_, _| 0.5);
        inputs[MidiOut::PORT_CC + 1] = cable(1, SignalType::Control, |_, _| 1.0);
        run(&mut module, &inputs, &params, TransportState::new());
        // Half of 16383 is 8192: 64 and 0
        assert_eq!(
            bytes(&mut ring),
            [vec![0xB0, 7, 64], vec![0xB0, 39, 0], vec![0xB0, 99, 1], vec![0xB0, 98, 20], vec![0xB0, 6, 127], vec![0xB0, 38, 127]]
        );
        assert!((module.readout(&params).unwrap().values[MidiOut::READOUT_CC] - 63.5).abs() < 0.01, "finer than 7 bits");
    }

    #[test]
    fn bend_moves_and_springs_back() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        params[MidiOut::PARAM_CHANNEL] = 1.0;
        let mut inputs = inputs(&module);
        inputs[MidiOut::PORT_BEND] = cable(1, SignalType::Control, |_, _| 1.0);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(bytes(&mut ring), [vec![0xE1, 0x7F, 0x7F]], "full up is 16383, LSB first");
        inputs[MidiOut::PORT_BEND] = cable(1, SignalType::Control, |_, _| -1.0);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(bytes(&mut ring), [vec![0xE1, 0, 0]]);
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_BEND], -1.0);
        inputs[MidiOut::PORT_BEND] = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(bytes(&mut ring), [vec![0xE1, 0x00, 0x40]], "unplugged: the middle, 8192");
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "once");
    }

    #[test]
    fn the_bend_range_is_told_with_rpn_0() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let inputs = inputs(&module);
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "off by default: the device's own");
        params[MidiOut::PARAM_BEND_RANGE] = 12.0;
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(
            bytes(&mut ring),
            [vec![0xB0, 101, 0], vec![0xB0, 100, 0], vec![0xB0, 6, 12], vec![0xB0, 38, 0], vec![0xB0, 101, 127], vec![0xB0, 100, 127]]
        );
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "once");
    }

    #[test]
    fn pressure_goes_by_channel_or_by_note() {
        let (mut module, mut ring) = module();
        let params = defaults(&module);
        let mut inputs = inputs(&module);
        inputs[MidiOut::PORT_PRESSURE] = cable(1, SignalType::Control, |_, _| 0.5);
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(bytes(&mut ring), [vec![0xD0, 64]], "Channel Pressure");
        // A poly cable on a two-note chord struck at sample 8: each note's own, after it
        let chord = [0.0, 7.0];
        inputs[MidiOut::PORT_PITCH] = cable(2, SignalType::Control, |c, _| chord[c] / 12.0);
        inputs[MidiOut::PORT_GATE] = cable(2, SignalType::Gate, |_, i| if i >= 8 { 1.0 } else { 0.0 });
        inputs[MidiOut::PORT_PRESSURE] = cable(2, SignalType::Control, |c, _| [0.25, 1.0][c]);
        run(&mut module, &inputs, &params, TransportState::new());
        let sent = messages(&mut ring);
        let after = |status: u8| sent.iter().filter(|(_, m)| m[0] == status).map(|(s, m)| (*s - BLOCK as u64, m.clone())).collect::<Vec<_>>();
        assert_eq!(after(0x90).len(), 2);
        assert_eq!(after(0xA0), [(8, vec![0xA0, 60, 32]), (8, vec![0xA0, 67, 127])]);
        let ons = sent.iter().position(|(_, m)| m[0] == 0x90).unwrap();
        let touch = sent.iter().position(|(_, m)| m[0] == 0xA0).unwrap();
        assert!(ons < touch, "the note first");
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_POLY], 1.0);
    }

    #[test]
    fn a_program_and_bank_go_on_change_and_on_send_program() {
        let (mut module, mut ring) = module();
        let mut params = defaults(&module);
        let mut inputs = inputs(&module);
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "off by default");
        params[MidiOut::PARAM_PROGRAM] = 4.0;
        params[MidiOut::PARAM_BANK_MSB] = 87.0;
        params[MidiOut::PARAM_BANK_LSB] = 1.0;
        params[MidiOut::PARAM_CHANNEL] = 9.0;
        run(&mut module, &inputs, &params, TransportState::new());
        let burst = vec![vec![0xB9, 0, 87], vec![0xB9, 32, 1], vec![0xC9, 4]];
        assert_eq!(bytes(&mut ring), burst);
        run(&mut module, &inputs, &params, TransportState::new());
        assert!(messages(&mut ring).is_empty(), "unchanged");
        inputs[MidiOut::PORT_SEND_PROGRAM] = cable(1, SignalType::Gate, |_, i| if i >= 30 { 1.0 } else { 0.0 });
        run(&mut module, &inputs, &params, TransportState::new());
        let again = messages(&mut ring);
        assert_eq!(again.iter().map(|(s, _)| *s - 3 * BLOCK as u64).collect::<Vec<_>>(), [30, 30, 30]);
        // No bank: the Program Change alone (the trigger held high)
        inputs[MidiOut::PORT_SEND_PROGRAM] = cable(1, SignalType::Gate, |_, _| 1.0);
        params[MidiOut::PARAM_BANK_MSB] = -1.0;
        params[MidiOut::PARAM_BANK_LSB] = -1.0;
        run(&mut module, &inputs, &params, TransportState::new());
        assert_eq!(bytes(&mut ring), [vec![0xC9, 4]]);
        assert_eq!(module.readout(&params).unwrap().values[MidiOut::READOUT_PROGRAMS_SENT], 3.0);
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
