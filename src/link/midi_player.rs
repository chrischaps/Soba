//! Notes, bend and pressure from cables, for any module that plays a
//! device (#439, #442): MIDI Out's, and an Instrument's. A gate and a pitch
//! cable become notes (a poly cable, chords), a bend cable Pitch Bend, and
//! a pressure cable Channel Pressure, or each note's Poly Pressure from a
//! poly cable. Everything here runs on the audio thread, and nothing
//! allocates.

use super::midi::{Control, Message, BEND_CENTER};
use super::midi_writer::{ControlLane, MidiWriter, WRAP};
use crate::dsp::gate::{Edge, Transition};
use crate::dsp::signal::{SignalBuffer, MAX_CHANNELS};

/// What a module's cables play on one channel of a device.
pub struct Player {
    /// The channel it plays on, from 0.
    channel: u8,
    /// Each voice's gate at the end of the last block.
    gates: [Edge; MAX_CHANNELS],
    /// The note each voice is holding.
    voice_note: [Option<u8>; MAX_CHANNELS],
    /// Where each voice's note began in this block: 0 for one held from before.
    voice_from: [usize; MAX_CHANNELS],
    /// How many voices hold each note: it's let go when the last does.
    held: [u8; 128],
    bend: ControlLane,
    pressure: ControlLane,
    /// Each voice's Poly Pressure, from a poly Pressure cable.
    poly_pressure: [ControlLane; MAX_CHANNELS],
    /// Whether the patch's Clock was playing last block.
    was_playing: bool,
    /// Notes sent so far, wrapping at [`WRAP`].
    notes_sent: u32,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub const fn new() -> Self {
        Self {
            channel: 0,
            gates: [Edge::LOW; MAX_CHANNELS],
            voice_note: [None; MAX_CHANNELS],
            voice_from: [0; MAX_CHANNELS],
            held: [0; 128],
            bend: ControlLane::new(),
            pressure: ControlLane::new(),
            poly_pressure: [ControlLane::new(); MAX_CHANNELS],
            was_playing: false,
            notes_sent: 0,
        }
    }

    /// The channel it plays on, from 0.
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// Starts a block on `channel`, with the patch's Clock `playing` or
    /// not. A new channel lets go of the notes and the bend on the old one,
    /// and a Clock that stops lets go of notes nothing else would.
    /// REAL-TIME SAFE.
    pub fn begin(&mut self, out: &mut MidiWriter, channel: u8, playing: bool) {
        if channel != self.channel {
            // Notes end, and the bend springs back, on the channel they started on
            self.release_all(out, 0);
            self.relax(out, 0);
            self.channel = channel;
        }
        self.voice_from = [0; MAX_CHANNELS];
        if self.was_playing && !playing {
            // Stopped mid-note: nothing will let go of it
            self.release_all(out, 0);
        }
        self.was_playing = playing;
    }

    /// Forgets what the bend and pressure sent, for a module moved to
    /// another port: the new device hears them afresh. The sender lets go
    /// of the notes, and the bend, on the old one.
    pub fn moved(&mut self) {
        self.bend.reset();
        self.pressure.reset();
        self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
    }

    /// Plays the notes: a voice per channel of the gate (or pitch) cable,
    /// each struck at its pitch (V/Oct, 0 is C4) and velocity (0 to 1) as
    /// its gate rises. REAL-TIME SAFE.
    pub fn notes(&mut self, out: &mut MidiWriter, gate: Option<&SignalBuffer>, pitch: Option<&SignalBuffer>, velocity: Option<&SignalBuffer>, frames: usize) {
        let voices = gate.map_or(0, |g| g.channels().max(pitch.map_or(1, |p| p.channels())));
        for voice in voices..MAX_CHANNELS {
            // A voice the cable no longer carries
            self.gates[voice].reset();
            self.note_off(out, voice, 0);
        }
        let Some(gate) = gate else { return };
        // Sample by sample across the voices, so messages go in time
        // order and a note two voices share ends with the later
        for offset in 0..frames {
            for voice in 0..voices {
                match self.gates[voice].tick(gate.voice(voice).at(offset)) {
                    None => continue,
                    Some(Transition::Falling) => {
                        self.note_off(out, voice, offset);
                        continue;
                    }
                    Some(Transition::Rising) => {}
                }
                let read = |input: Option<&SignalBuffer>, default: f32| input.map_or(default, |i| i.voice(voice).at(offset));
                let note = (60.0 + read(pitch, 0.0) * 12.0).round().clamp(0.0, 127.0) as u8;
                let strike = (read(velocity, 0.8) * 127.0).round().clamp(1.0, 127.0) as u8;
                self.note_on(out, voice, note, strike, offset);
            }
        }
    }

    /// Sends the bend (-1 to 1) where it moves; unplugged, it springs back.
    /// REAL-TIME SAFE.
    pub fn bend(&mut self, out: &mut MidiWriter, bend: Option<&SignalBuffer>, frames: usize, gap: usize) {
        let channel = self.channel;
        match bend {
            Some(bend) => self.bend.run(out, 0..frames, gap, |i| Control::bend(channel, bend.at(i))),
            None => {
                if matches!(self.bend.sent(), Some(Control::Bend { value, .. }) if value != BEND_CENTER) {
                    out.control(0, Control::Bend { channel, value: BEND_CENTER });
                }
                self.bend.reset();
            }
        }
    }

    /// Sends the pressure (0 to 1) where it moves: the channel's from one
    /// cable, each note's from a poly one. REAL-TIME SAFE.
    pub fn pressure(&mut self, out: &mut MidiWriter, pressure: Option<&SignalBuffer>, frames: usize, gap: usize) {
        let channel = self.channel;
        match pressure {
            Some(pressure) if pressure.channels() > 1 => {
                self.pressure.reset();
                for voice in 0..MAX_CHANNELS {
                    let Some(note) = self.voice_note[voice].filter(|_| voice < pressure.channels()) else {
                        self.poly_pressure[voice].reset();
                        continue;
                    };
                    let cable = pressure.voice(voice);
                    let from = self.voice_from[voice];
                    self.poly_pressure[voice].run(out, from..frames, gap, |i| Control::PolyPressure { channel, note, value: Control::seven(cable.at(i)) });
                }
            }
            Some(pressure) => {
                self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
                self.pressure.run(out, 0..frames, gap, |i| Control::Pressure { channel, value: Control::seven(pressure.at(i)) });
            }
            None => {
                if matches!(self.pressure.sent(), Some(Control::Pressure { value, .. }) if value > 0) {
                    out.control(0, Control::Pressure { channel, value: 0 });
                }
                self.pressure.reset();
                self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
            }
        }
    }

    /// Lets go of every note, springs the bend back and the pressure off,
    /// and forgets the gates. REAL-TIME SAFE.
    pub fn reset(&mut self, out: &mut MidiWriter) {
        self.release_all(out, 0);
        self.relax(out, 0);
        self.gates = [Edge::LOW; MAX_CHANNELS];
    }

    /// Whether note `note` is held.
    pub fn holds(&self, note: u8) -> bool {
        self.held[note as usize & 0x7F] > 0
    }

    /// Whether the patch's Clock was playing last block.
    pub fn playing(&self) -> bool {
        self.was_playing
    }

    /// Notes sent so far, wrapping at [`WRAP`].
    pub fn notes_sent(&self) -> u32 {
        self.notes_sent
    }

    /// The bend as last sent, -1 to 1, or `None` while unpatched.
    pub fn bend_sent(&self) -> Option<f32> {
        match self.bend.sent() {
            Some(Control::Bend { value, .. }) => Some((value as f32 - BEND_CENTER as f32) / BEND_CENTER as f32),
            _ => None,
        }
    }

    /// The pressure as last sent, 0 to 1 (a poly cable's highest), and
    /// whether it goes note by note; `None` while unpatched.
    pub fn pressure_sent(&self) -> Option<(f32, bool)> {
        if let Some(Control::Pressure { value, .. }) = self.pressure.sent() {
            return Some((value as f32 / 127.0, false));
        }
        let poly = self.poly_pressure.iter().filter_map(|lane| match lane.sent() {
            Some(Control::PolyPressure { value, .. }) => Some(value),
            _ => None,
        });
        poly.max().map(|value| (value as f32 / 127.0, true))
    }

    fn note_on(&mut self, out: &mut MidiWriter, voice: usize, note: u8, velocity: u8, offset: usize) {
        self.note_off(out, voice, offset);
        out.message(offset, Message::note_on(self.channel, note, velocity));
        self.held[note as usize] = self.held[note as usize].saturating_add(1);
        self.voice_note[voice] = Some(note);
        self.voice_from[voice] = offset;
        // Its pressure goes after it, afresh
        self.poly_pressure[voice].reset();
        self.notes_sent = (self.notes_sent + 1) % WRAP;
    }

    fn note_off(&mut self, out: &mut MidiWriter, voice: usize, offset: usize) {
        let Some(note) = self.voice_note[voice].take() else { return };
        let count = &mut self.held[note as usize];
        *count = count.saturating_sub(1);
        if *count == 0 {
            out.message(offset, Message::note_off(self.channel, note));
        }
    }

    /// Lets go of every note held. Gates still high stay quiet until they
    /// rise again.
    fn release_all(&mut self, out: &mut MidiWriter, offset: usize) {
        for note in 0..128u8 {
            if std::mem::take(&mut self.held[note as usize]) > 0 {
                out.message(offset, Message::note_off(self.channel, note));
            }
        }
        self.voice_note = [None; MAX_CHANNELS];
    }

    /// Lets the bend spring back and the pressure off, on the channel
    /// they went out on, if they're away from rest.
    fn relax(&mut self, out: &mut MidiWriter, offset: usize) {
        if matches!(self.bend.sent(), Some(Control::Bend { value, .. }) if value != BEND_CENTER) {
            out.control(offset, Control::Bend { channel: self.channel, value: BEND_CENTER });
        }
        if matches!(self.pressure.sent(), Some(Control::Pressure { value, .. }) if value > 0) {
            out.control(offset, Control::Pressure { channel: self.channel, value: 0 });
        }
        self.bend.reset();
        self.pressure.reset();
        self.poly_pressure = [ControlLane::new(); MAX_CHANNELS];
    }
}
