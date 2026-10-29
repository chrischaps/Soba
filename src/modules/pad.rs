//! Pad: one pad of a pad controller as a gate, for finger drums and
//! triggers (#480).
//!
//! Pads send notes, and played into a Note In they're pitches: pad 1 is C2.
//! A Pad hears one of them as a gate instead, to strike a Drum, open an
//! envelope or reset a sequencer, with how hard it was hit and how hard
//! it's pressed. Eight of them in a group are a pad controller's pads: the
//! Library's **Pads ×8** is the General MIDI drum notes, 36 to 43 on
//! channel 10, as most controllers send them. A Launchpad's 64 or a
//! footswitch's one are as many Pads.
//!
//! Its Note and Channel are hidden parameters, written by a learn from the
//! face (see [`crate::app::module_learn`]), so the module reads MIDI on the
//! audio thread, sample-accurately, and the patch keeps what it learned.
//!
//! **Any Note** has it hear every note on its channel (#469): a drum
//! machine's track sends its trig's own pitch, so a pitched trig is
//! another note, and a pad listening for the track's note would miss it.
//! A group of Pads built from a kit profile (**Pads for ▸**) sets it on
//! every pad whose part has a channel to itself, as the Digitakt's tracks
//! do.
//!
//! Its note is claimed: a Note In on Omni skips it (see
//! [`crate::dsp::note_claims`]), so a hit never also plays the synth.
//!
//! It took the place of Pads (#428), eight pads in one node: a patch that
//! has one opens with a group of eight Pads where it was (see
//! `crate::persistence::patch`).

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    primitives::rate_for_seconds,
    signal::SignalBuffer,
    ListenTo, MidiEvent, MidiMessage, NoteClaims, ParameterDisplay, SignalType, KEYS_CHANNEL,
};

/// The module's ID, as patches name it.
pub const ID: &str = "input.pad";

/// The note a Pad hears when it's added: C1, the General MIDI kick.
pub const DEFAULT_NOTE: u8 = 36;

/// The channel a Pad is heard on when it's added: 10, the General MIDI drum
/// channel.
pub const DEFAULT_CHANNEL: u8 = 10;

/// How long its gate lasts in Trigger mode, in seconds.
pub const TRIGGER_SECONDS: f32 = 0.005;

/// How long Pressure takes to follow the pad, so a 7-bit pressure's steps
/// don't zipper whatever it opens.
const PRESSURE_SLEW: f32 = 0.005;

/// The longest a pad is ever said to have been quiet, in seconds: the
/// readout's "never hit".
const QUIET: f32 = 1.0e6;

/// The Channel choices: 0 is Omni.
pub const CHANNEL_LABELS: &[&str] = &["Omni", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

/// How its gate follows its hits, as the Mode parameter holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// High while the pad is held.
    Momentary,
    /// Each hit flips it: hit once to open it, again to close it.
    Latch,
    /// A short gate per hit, however long the pad is held.
    Trigger,
}

impl Mode {
    /// The Mode choices, in parameter order.
    pub const LABELS: &'static [&'static str] = &["Momentary", "Latch", "Trigger"];
    const ALL: [Mode; 3] = [Mode::Momentary, Mode::Latch, Mode::Trigger];

    /// The mode a Mode parameter's value names.
    pub fn from_param(value: f32) -> Self {
        Self::ALL[(value.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

/// What a Pad listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listen {
    pub note: u8,
    /// 0 is Omni, 1-16 a MIDI channel.
    pub channel: u8,
    /// It hears every note on its channel, not just `note` (#469).
    pub any: bool,
}

impl Listen {
    /// Whether an event on wire channel `channel` (0-15, or the keys') of
    /// note `note` is this pad's.
    #[inline]
    fn hears(&self, channel: u8, note: u8) -> bool {
        (self.any || self.note == note) && self.hears_channel(channel)
    }

    /// Whether channel pressure on wire channel `channel` reaches it.
    #[inline]
    fn hears_channel(&self, channel: u8) -> bool {
        channel != KEYS_CHANNEL && (self.channel == 0 || self.channel - 1 == channel)
    }
}

/// What a Pad listens to, read from its parameters.
pub fn listen(params: &[f32]) -> Listen {
    let at = |index: usize, default: u8| params.get(index).copied().unwrap_or(default as f32).round();
    Listen {
        note: at(Pad::PARAM_NOTE, DEFAULT_NOTE).clamp(0.0, 127.0) as u8,
        channel: at(Pad::PARAM_CHANNEL, DEFAULT_CHANNEL).clamp(0.0, 16.0) as u8,
        any: at(Pad::PARAM_ANY, 0) >= 0.5,
    }
}

/// Claims the note a Pad with `params`, set to hear `device`, listens to,
/// so an Omni Note In skips it: from that device alone (#454), and every
/// note of its channel on Any Note. It's how the engine and the UI both
/// work it out.
///
/// REAL-TIME SAFE.
pub fn claim_notes(params: &[f32], device: ListenTo, claims: &mut NoteClaims) {
    let listen = listen(params);
    let channel = listen.channel.checked_sub(1);
    if listen.any {
        claims.claim_all(device, channel);
    } else {
        claims.claim(device, channel, listen.note);
    }
}

/// One pad from a pad controller, as a gate.
///
/// # Ports
///
/// **Inputs:**
/// - **MIDI** (MIDI): from a MIDI In, the device and channel it hears
///   (#479); unpatched, its own Device.
///
/// **Outputs:**
/// - **Gate** (Gate): the pad, as Mode says.
/// - **Velocity** (Control): how hard it was last hit, 0 to 1, from the
///   sample its gate rises.
/// - **Pressure** (Control): how hard it's pressed, 0 to 1, from its own
///   polyphonic aftertouch or the channel's while it's held.
///
/// # Parameters
///
/// - **Mode**: Momentary (high while held), Latch (each hit flips it) or
///   Trigger (a 5 ms gate per hit).
/// - Hidden, set from the face: **Note** and **Channel**, which a learn
///   writes, **Press**, the pad pressed on the face, and **Any Note**, on
///   which it hears every note of its channel.
pub struct Pad {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    /// Held down on the controller.
    held: bool,
    /// Held down on the face.
    pressed: bool,
    /// Open, in Latch mode.
    latched: bool,
    /// Samples of a Trigger's gate still to come.
    trigger_left: u32,
    /// Samples its gate drops for before rising again, so a hit while it's
    /// high is a new edge.
    rest: u32,
    /// How hard it was last hit, 0 to 1.
    velocity: f32,
    /// How hard it's pressed, from polyphonic aftertouch, 0 to 1.
    pad_pressure: f32,
    /// Channel pressure, while it's held.
    channel_pressure: f32,
    /// The Pressure output, as slewed.
    pressure: f32,
    /// Seconds since it was last hit.
    quiet: f32,
    /// Its gate at the end of the last block, for the face.
    gate: bool,
    /// The face's press as the last block saw it.
    last_press: f32,
    /// The mode the last block played, so a new one starts clean.
    mode: Mode,
    sample_rate: f32,
}

impl Pad {
    /// Creates a new Pad.
    pub fn new() -> Self {
        // The device and channel it hears, from a MIDI In (#479). Inputs
        // come first: the editor counts ports so
        let ports = vec![
            super::midi_in::input(),
            PortDefinition::output("gate", "Gate", SignalType::Gate)
                .describe("High while the pad is held, flipped by each hit, or a short trigger, as Mode says"),
            PortDefinition::output("velocity", "Velocity", SignalType::Control)
                .describe("How hard the pad was last hit, 0 to 1, from the sample its gate rises: patch it into a drum's Accent"),
            PortDefinition::output("pressure", "Pressure", SignalType::Control)
                .describe("How hard the pad is pressed (aftertouch), 0 to 1, while it's held"),
        ];
        let parameters = vec![
            ParameterDefinition::choice("mode", "Mode", Mode::LABELS, 0)
                .describe("Momentary is high while the pad is held; Latch flips with each hit; Trigger is a short gate per hit"),
            ParameterDefinition::new("note", "Note", 0.0, 127.0, DEFAULT_NOTE as f32, ParameterDisplay::stepped(""))
                .describe("The note the pad sends, learned from the face"),
            ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, DEFAULT_CHANNEL as usize)
                .describe("The MIDI channel the pad is heard on; Omni hears all"),
            ParameterDefinition::new("press", "Press", 0.0, 1.0, 0.0, ParameterDisplay::linear(""))
                .describe("The pad pressed on the face, as hard as this"),
            ParameterDefinition::toggle("any_note", "Any Note", false)
                .describe("Hear every note on the channel, not just Note: a drum machine's track, pitched trigs and all"),
        ];
        Self {
            ports,
            parameters,
            held: false,
            pressed: false,
            latched: false,
            trigger_left: 0,
            rest: 0,
            velocity: 0.0,
            pad_pressure: 0.0,
            channel_pressure: 0.0,
            pressure: 0.0,
            quiet: QUIET,
            gate: false,
            last_press: 0.0,
            mode: Mode::Momentary,
            sample_rate: 44100.0,
        }
    }

    /// The Mode parameter's index.
    pub const PARAM_MODE: usize = 0;
    /// The Note parameter's index: a learn writes the note heard here.
    pub const PARAM_NOTE: usize = 1;
    /// The Channel parameter's index: 0 Omni, 1-16.
    pub const PARAM_CHANNEL: usize = 2;
    /// The Press parameter's index, which the face plays.
    pub const PARAM_PRESS: usize = 3;
    /// The Any Note parameter's index: on, it hears every note of its
    /// channel.
    pub const PARAM_ANY: usize = 4;

    /// The output index of Gate, Velocity and Pressure.
    pub const PORT_GATE: usize = 0;
    pub const PORT_VELOCITY: usize = 1;
    pub const PORT_PRESSURE: usize = 2;

    /// The readout's places: its gate (0 or 1), how hard it was last hit,
    /// the seconds since, and how hard it's pressed.
    pub const READOUT_GATE: usize = 0;
    pub const READOUT_VELOCITY: usize = 1;
    pub const READOUT_QUIET: usize = 2;
    pub const READOUT_PRESSURE: usize = 3;

    fn down(&self) -> bool {
        self.held || self.pressed
    }

    /// Whether its gate is high, in `mode`, before any rest.
    fn open(&self, mode: Mode) -> bool {
        match mode {
            Mode::Momentary => self.down(),
            Mode::Latch => self.latched,
            Mode::Trigger => self.trigger_left > 0,
        }
    }

    /// Strikes it at `velocity` (0 to 1), held down on the face or on the
    /// controller.
    fn hit(&mut self, velocity: f32, mode: Mode, trigger: u32, on_face: bool) {
        let was_open = self.open(mode) && self.rest == 0;
        if on_face {
            self.pressed = true;
        } else {
            self.held = true;
        }
        match mode {
            Mode::Momentary => {
                if was_open {
                    self.rest = 1;
                }
            }
            Mode::Latch => self.latched = !self.latched,
            Mode::Trigger => {
                if was_open {
                    self.rest = 1;
                }
                self.trigger_left = trigger;
            }
        }
        self.velocity = velocity;
        self.pad_pressure = 0.0;
        self.quiet = 0.0;
    }

    /// Hears `event`: a hit if it's its note, or whatever else it says.
    fn take(&mut self, event: &MidiEvent, listen: Listen, mode: Mode, trigger: u32) {
        match event.message {
            MidiMessage::NoteOn { note, velocity } if velocity > 0 => {
                if listen.hears(event.channel, note) {
                    self.hit(velocity as f32 / 127.0, mode, trigger, false);
                }
            }
            MidiMessage::NoteOff { note, .. } | MidiMessage::NoteOn { note, .. } => {
                if listen.hears(event.channel, note) {
                    self.held = false;
                    self.pad_pressure = 0.0;
                }
            }
            MidiMessage::PolyPressure { note, pressure } => {
                if listen.hears(event.channel, note) {
                    self.pad_pressure = pressure as f32 / 127.0;
                }
            }
            MidiMessage::Aftertouch { pressure } => {
                if listen.hears_channel(event.channel) {
                    self.channel_pressure = pressure as f32 / 127.0;
                }
            }
            MidiMessage::ControlChange { controller: 120 | 123, .. } => {
                // All Sound Off, All Notes Off: it lets go
                self.held = false;
                self.pad_pressure = 0.0;
            }
            _ => {}
        }
    }

    /// How hard it's pressed now: its own pressure, or the channel's.
    fn pressed_by(&self) -> f32 {
        if self.down() { self.pad_pressure.max(self.channel_pressure) } else { 0.0 }
    }
}

impl Default for Pad {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Pad {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Pad",
            category: ModuleCategory::Source,
            description: "One MIDI pad from a pad controller as a gate, with velocity and pressure: finger drums, triggers and latches",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn note_claims(&self) -> Option<crate::dsp::ClaimNotes> {
        Some(claim_notes)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
    }

    fn process(&mut self, _inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let mode = Mode::from_param(params.get(Self::PARAM_MODE).copied().unwrap_or(0.0));
        if mode != self.mode {
            // A new mode starts with its gate shut
            self.mode = mode;
            self.latched = false;
            self.trigger_left = 0;
            self.rest = 0;
        }
        let listen = listen(params);
        let trigger = ((TRIGGER_SECONDS * self.sample_rate).round() as u32).max(1);
        let slew = rate_for_seconds(PRESSURE_SLEW, self.sample_rate);
        self.quiet = (self.quiet + context.block_size as f32 / self.sample_rate).min(QUIET);

        // Pressed on the face, or let go, from the block's first sample
        let press = params.get(Self::PARAM_PRESS).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if press > 0.0 && self.last_press == 0.0 {
            self.hit(press, mode, trigger, true);
        } else if press == 0.0 && self.last_press > 0.0 {
            self.pressed = false;
        }
        self.last_press = press;

        for output in outputs.iter_mut() {
            output.set_channels(1);
        }
        let mut events = context.midi.iter().peekable();
        for i in 0..context.block_size {
            while let Some(event) = events.next_if(|event| event.sample_offset as usize <= i) {
                self.take(event, listen, mode, trigger);
            }
            let gate = self.rest == 0 && self.open(mode);
            self.rest = self.rest.saturating_sub(1);
            self.trigger_left = self.trigger_left.saturating_sub(1);
            if !self.down() {
                self.channel_pressure = 0.0;
            }
            self.pressure += (self.pressed_by() - self.pressure) * slew;
            self.gate = gate;
            for (port, value) in [(Self::PORT_GATE, if gate { 1.0 } else { 0.0 }), (Self::PORT_VELOCITY, self.velocity), (Self::PORT_PRESSURE, self.pressure)] {
                if let Some(output) = outputs.get_mut(port) {
                    output.samples[i] = value;
                }
            }
        }
        // Anything placed past the block still counts, from the next one
        for event in events {
            self.take(event, listen, mode, trigger);
        }
    }

    fn reset(&mut self) {
        // What's held stays held; the rest goes quiet
        self.latched = false;
        self.trigger_left = 0;
        self.rest = 0;
        self.velocity = 0.0;
        self.pad_pressure = 0.0;
        self.channel_pressure = 0.0;
        self.pressure = 0.0;
        self.quiet = QUIET;
        self.gate = false;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_GATE] = if self.gate { 1.0 } else { 0.0 };
        readout.values[Self::READOUT_VELOCITY] = self.velocity;
        readout.values[Self::READOUT_QUIET] = self.quiet;
        readout.values[Self::READOUT_PRESSURE] = self.pressed_by();
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 256;
    /// Channel 10, as the wire has it.
    const TEN: u8 = 9;

    fn defaults() -> Vec<f32> {
        Pad::new().parameters().iter().map(|p| p.default).collect()
    }

    fn on_note(note: u8) -> Vec<f32> {
        let mut params = defaults();
        params[Pad::PARAM_NOTE] = note as f32;
        params
    }

    fn module() -> Pad {
        let mut module = Pad::new();
        module.prepare(SR, BLOCK);
        module
    }

    fn run(module: &mut Pad, params: &[f32], midi: &[MidiEvent]) -> Vec<SignalBuffer> {
        let mut out = vec![
            SignalBuffer::new(BLOCK, SignalType::Gate),
            SignalBuffer::new(BLOCK, SignalType::Control),
            SignalBuffer::new(BLOCK, SignalType::Control),
        ];
        let context = ProcessContext::new(SR, BLOCK).with_midi(midi);
        module.process(&[], &mut out, params, &context);
        out
    }

    fn on(at: u32, channel: u8, note: u8, velocity: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::NoteOn { note, velocity })
    }

    fn off(at: u32, channel: u8, note: u8) -> MidiEvent {
        MidiEvent::new(at, channel, MidiMessage::NoteOff { note, velocity: 0 })
    }

    /// The samples at which `samples` rises.
    fn rises(samples: &[f32]) -> Vec<usize> {
        let mut last = 0.0;
        let mut at = Vec::new();
        for (i, &s) in samples.iter().enumerate() {
            if s > 0.5 && last <= 0.5 {
                at.push(i);
            }
            last = s;
        }
        at
    }

    #[test]
    fn info_ports_and_parameters() {
        let pad = Pad::new();
        assert_eq!(pad.info().id, "input.pad");
        let names: Vec<&str> = pad.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["MIDI", "Gate", "Velocity", "Pressure"], "the MIDI input first (#479)");
        let params: Vec<&str> = pad.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params, ["Mode", "Note", "Channel", "Press", "Any Note"]);
        for word in ["pad", "MIDI", "drums", "trigger"] {
            assert!(pad.info().description.contains(word), "Quick Add finds it under '{word}'");
        }
    }

    #[test]
    fn a_new_pad_is_the_gm_kick_on_channel_ten() {
        assert_eq!(listen(&defaults()), Listen { note: 36, channel: 10, any: false });
        let mut module = module();
        let out = run(&mut module, &on_note(38), &[on(10, TEN, 38, 100), on(20, 0, 38, 100), on(30, TEN, 39, 100)]);
        assert_eq!(rises(&out[Pad::PORT_GATE].samples), [10], "the snare, on channel 10 only");
    }

    #[test]
    fn on_any_note_it_hears_every_note_of_its_channel_and_claims_them() {
        // The Digitakt's Track 3: its trigs on channel 3, a pitched one too
        let mut params = on_note(2);
        params[Pad::PARAM_CHANNEL] = 3.0;
        params[Pad::PARAM_ANY] = 1.0;
        assert_eq!(listen(&params), Listen { note: 2, channel: 3, any: true });
        let mut module = module();
        let out = run(&mut module, &params, &[on(10, 2, 2, 100), off(20, 2, 2), on(40, 2, 67, 90), off(50, 2, 67), on(70, 3, 2, 100), on(90, 2, 0, 0)]);
        let gate = &out[Pad::PORT_GATE].samples;
        assert_eq!(rises(gate), [10, 40], "its own note, then a pitched trig; channel 4's is another track's");
        assert!(gate[50..].iter().all(|&s| s == 0.0), "let go by its note off");

        let mut claims = NoteClaims::default();
        claim_notes(&params, ListenTo::Any, &mut claims);
        assert!((0..128).all(|note| claims.claims(0, 2, note)), "an Omni Note In skips the whole channel");
        assert!(!claims.claims(0, 3, 2));
        params[Pad::PARAM_ANY] = 0.0;
        let mut claims = NoteClaims::default();
        claim_notes(&params, ListenTo::Any, &mut claims);
        assert!(claims.claims(0, 2, 2) && !claims.claims(0, 2, 67), "off, just its note");
    }

    #[test]
    fn momentary_holds_while_the_pad_is_held_and_a_rehit_is_a_new_edge() {
        let mut module = module();
        let out = run(&mut module, &defaults(), &[on(10, TEN, 36, 100), off(100, TEN, 36), on(150, TEN, 36, 100), on(200, TEN, 36, 60)]);
        let gate = &out[Pad::PORT_GATE].samples;
        assert_eq!(gate[9], 0.0);
        assert!(gate[10..100].iter().all(|&s| s == 1.0), "high while held");
        assert!(gate[100..150].iter().all(|&s| s == 0.0), "low once let go");
        assert_eq!(rises(gate), [10, 150, 201], "struck again while held, it drops for a sample and rises");
        assert!((out[Pad::PORT_VELOCITY].samples[BLOCK - 1] - 60.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn a_latch_beside_a_momentary_each_follow_their_own_mode() {
        // The acceptance: a Pad in Latch opens on one hit and closes on the
        // next, while one beside it in Momentary follows its pad
        let (mut latch, mut momentary) = (module(), module());
        let mut latch_params = on_note(37);
        latch_params[Pad::PARAM_MODE] = 1.0;
        let momentary_params = on_note(38);
        let midi = [on(10, TEN, 37, 100), on(10, TEN, 38, 100), off(20, TEN, 37), off(20, TEN, 38), on(100, TEN, 37, 100), on(150, TEN, 38, 90)];
        let latched = run(&mut latch, &latch_params, &midi);
        let followed = run(&mut momentary, &momentary_params, &midi);
        let gate = &latched[Pad::PORT_GATE].samples;
        assert!(gate[10..100].iter().all(|&s| s == 1.0), "open past its release");
        assert!(gate[100..].iter().all(|&s| s == 0.0), "the next hit closes it");
        let gate = &followed[Pad::PORT_GATE].samples;
        assert!(gate[10..20].iter().all(|&s| s == 1.0) && gate[20..150].iter().all(|&s| s == 0.0) && gate[150] == 1.0);

        // Held down through the next block, a latch stays as the hit left it
        let out = run(&mut latch, &latch_params, &[on(0, TEN, 37, 100)]);
        assert!(out[Pad::PORT_GATE].samples.iter().all(|&s| s == 1.0));
        let out = run(&mut latch, &latch_params, &[]);
        assert!(out[Pad::PORT_GATE].samples.iter().all(|&s| s == 1.0));
        assert_eq!(latch.readout(&latch_params).unwrap().values[Pad::READOUT_GATE], 1.0, "the face shows it open");
    }

    #[test]
    fn trigger_is_a_short_gate_per_hit_however_long_it_is_held() {
        let mut module = module();
        let mut params = defaults();
        params[Pad::PARAM_MODE] = 2.0;
        let long = (TRIGGER_SECONDS * SR) as usize;
        let mut gate = Vec::new();
        gate.extend(run(&mut module, &params, &[on(10, TEN, 36, 100)])[0].samples.iter());
        gate.extend(run(&mut module, &params, &[])[0].samples.iter());
        assert_eq!(rises(&gate), [10]);
        assert_eq!(gate.iter().filter(|&&s| s == 1.0).count(), long, "5 ms, held or not");
        // Struck again inside its gate, it starts over on a new edge
        let out = run(&mut module, &params, &[on(0, TEN, 36, 100), on(100, TEN, 36, 100)]);
        assert_eq!(rises(&out[0].samples), [0, 101]);
    }

    #[test]
    fn two_pads_struck_on_one_sample_each_put_out_their_own_velocity_there() {
        // Kick and snare together, as a two-finger hit sends them: no
        // stagger, and no shared Hit to stagger for
        let (mut kick, mut snare) = (module(), module());
        let midi = [on(20, TEN, 36, 127), on(20, TEN, 38, 40)];
        let kicked = run(&mut kick, &on_note(36), &midi);
        let snared = run(&mut snare, &on_note(38), &midi);
        assert_eq!(rises(&kicked[Pad::PORT_GATE].samples), [20]);
        assert_eq!(rises(&snared[Pad::PORT_GATE].samples), [20]);
        assert_eq!(kicked[Pad::PORT_VELOCITY].samples[19], 0.0);
        assert_eq!(kicked[Pad::PORT_VELOCITY].samples[20], 1.0);
        assert!((snared[Pad::PORT_VELOCITY].samples[20] - 40.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn pressure_is_its_own_or_the_channels_while_held_and_falls_when_let_go() {
        let mut module = module();
        let params = defaults();
        let poly = |at, note, pressure| MidiEvent::new(at, TEN, MidiMessage::PolyPressure { note, pressure });
        let settled = |module: &mut Pad| {
            let mut last = 0.0;
            for _ in 0..20 {
                last = run(module, &params, &[])[Pad::PORT_PRESSURE].samples[BLOCK - 1];
            }
            last
        };
        run(&mut module, &params, &[on(0, TEN, 36, 100), poly(2, 36, 100), poly(3, 37, 127)]);
        assert!((settled(&mut module) - 100.0 / 127.0).abs() < 1e-3, "its own, not its neighbour's");
        assert!((module.readout(&params).unwrap().values[Pad::READOUT_PRESSURE] - 100.0 / 127.0).abs() < 1e-6);
        // Channel pressure on its channel presses it while it's held
        run(&mut module, &params, &[MidiEvent::new(0, TEN, MidiMessage::Aftertouch { pressure: 127 }), MidiEvent::new(1, 0, MidiMessage::Aftertouch { pressure: 0 })]);
        assert!((settled(&mut module) - 1.0).abs() < 1e-3);
        run(&mut module, &params, &[off(0, TEN, 36)]);
        assert!(settled(&mut module) < 1e-3, "let go, nothing pressed");
        // Nor does channel pressure press it while it isn't held
        run(&mut module, &params, &[MidiEvent::new(0, TEN, MidiMessage::Aftertouch { pressure: 127 })]);
        assert!(settled(&mut module) < 1e-3);
    }

    #[test]
    fn a_press_on_the_face_plays_the_pad() {
        let mut module = module();
        let mut params = defaults();
        params[Pad::PARAM_PRESS] = 0.8;
        let out = run(&mut module, &params, &[]);
        assert_eq!(rises(&out[0].samples), [0]);
        assert_eq!(out[Pad::PORT_VELOCITY].samples[0], 0.8);
        assert_eq!(module.readout(&params).unwrap().values[Pad::READOUT_QUIET], 0.0, "just hit");
        let out = run(&mut module, &params, &[]);
        assert!(out[0].samples.iter().all(|&s| s == 1.0), "held while it's down");
        params[Pad::PARAM_PRESS] = 0.0;
        let out = run(&mut module, &params, &[]);
        assert!(out[0].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn the_keys_never_play_it_and_omni_hears_every_channel() {
        let mut module = module();
        let mut params = defaults();
        params[Pad::PARAM_CHANNEL] = 0.0;
        let out = run(&mut module, &params, &[on(0, KEYS_CHANNEL, 36, 100), on(1, 3, 36, 100)]);
        assert_eq!(rises(&out[0].samples), [1], "the computer keys play Note In");
    }

    #[test]
    fn reset_lets_its_gates_go_but_keeps_what_is_held() {
        let mut module = module();
        let mut params = defaults();
        params[Pad::PARAM_MODE] = 1.0;
        run(&mut module, &params, &[on(0, TEN, 36, 100)]);
        module.reset();
        assert_eq!(module.ports().len(), 4, "its ports kept");
        assert!(module.held && !module.latched);
        let out = run(&mut module, &params, &[]);
        assert!(out[0].samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn it_claims_its_note_on_its_channel() {
        let mut params = on_note(40);
        let mut claims = NoteClaims::EMPTY;
        claim_notes(&params, ListenTo::Any, &mut claims);
        assert!(claims.claims(0, TEN, 40));
        assert!(!claims.claims(0, TEN, 41) && !claims.claims(0, 0, 40));
        params[Pad::PARAM_CHANNEL] = 0.0;
        let mut claims = NoteClaims::EMPTY;
        claim_notes(&params, ListenTo::Any, &mut claims);
        assert!((0..16).all(|channel| claims.claims(0, channel, 40)), "on Omni it claims its note everywhere");

        // Set to one device, the claim is that device's (#454)
        let mut claims = NoteClaims::EMPTY;
        claim_notes(&on_note(36), ListenTo::Port(1), &mut claims);
        assert!(claims.claims(1, TEN, 36) && !claims.claims(0, TEN, 36));
    }
}
