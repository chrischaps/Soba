//! The send path a module plays hardware through (#439): MIDI Out's, and
//! any other module that sends MIDI (the Instrument, #442; the Setlist,
//! #445). Everything here runs on the audio thread, and nothing allocates.
//!
//! - [`MidiWriter`] is a module's end of its ring to the sender: it says
//!   when each block began, and stamps every message with its sample.
//! - [`ControlLane`] turns a cable into a controller, sent at the samples
//!   where its value changes, no more often than once every
//!   [`LANE_GAP`]: a CC lane, the bend, the pressure.
//! - [`ProgramSlot`] sends a Bank Select and Program Change when they
//!   change, and again on a trigger.
//!
//! What a writer sends, the sender sends to the port's device when its
//! sample is heard, within the port's budget (see [`MidiBus::set_budget`]):
//! [`Control`]s may be thinned while the port is full, plain [`Message`]s
//! (notes, programs) never are.

use std::ops::Range;

use rtrb::Producer;
use web_time::Instant;

use super::midi::{Control, Message, MidiBus, Outgoing, Program, PORTS};
use crate::dsp::context::ProcessContext;
use crate::dsp::gate::Edge;

/// The least time between two values of one lane, in seconds: a thousand
/// a second at most, which a USB device takes and a DIN port's budget
/// thins (#439).
pub const LANE_GAP: f32 = 0.001;

/// Counts a writer keeps wrap here, well inside an f32's whole numbers,
/// for a module's readout.
pub const WRAP: u32 = 4096;

/// A module's ring to the sender.
pub struct MidiWriter {
    ring: Option<Producer<Outgoing>>,
    /// Samples run so far: the clock its messages are stamped on.
    sample: u64,
    /// Events the ring had no room for, wrapping at [`WRAP`].
    dropped: u32,
}

impl MidiWriter {
    /// A writer to a new ring from `bus`, or one that sends nothing while
    /// no sender drains it (the offline renderer). On the UI thread, as
    /// the module is made: the ring is allocated here.
    pub fn on_bus(bus: &MidiBus) -> Self {
        Self { ring: bus.open_stream(), sample: 0, dropped: 0 }
    }

    /// Whether a sender reads what this writes.
    pub fn is_live(&self) -> bool {
        self.ring.is_some()
    }

    /// Starts a block on `port`: the sender learns when its first sample
    /// will be heard, and which port this module plays now. Call it first
    /// in `process`, before anything is sent. REAL-TIME SAFE.
    pub fn begin(&mut self, context: &ProcessContext, port: usize) {
        let Some(ring) = &mut self.ring else { return };
        // A block not played as it's made (a capture) goes out as it's made
        let at = context.sent_at.unwrap_or_else(Instant::now);
        let block = Outgoing::Block { sample: self.sample, at, rate: context.sample_rate, port: port.min(PORTS - 1) as u8 };
        if ring.push(block).is_err() {
            self.dropped = (self.dropped + 1) % WRAP;
        }
    }

    /// Ends a block `frames` long. REAL-TIME SAFE.
    pub fn end(&mut self, frames: usize) {
        self.sample += frames as u64;
    }

    /// Sends `message` at `offset` samples into this block. Notes and
    /// programs go this way: no budget holds them back. REAL-TIME SAFE.
    pub fn message(&mut self, offset: usize, message: Message) {
        self.push(Outgoing::Message { sample: self.sample + offset as u64, message });
    }

    /// Sends a controller's value at `offset` samples into this block.
    /// While the port is full, a newer value of the same controller may
    /// take its place. REAL-TIME SAFE.
    pub fn control(&mut self, offset: usize, control: Control) {
        self.push(Outgoing::Control { sample: self.sample + offset as u64, control });
    }

    /// Sends a Bank Select and Program Change on `channel`, in order, at
    /// `offset` samples into this block. REAL-TIME SAFE.
    pub fn program(&mut self, offset: usize, channel: u8, program: Program) {
        for &message in program.burst(channel).messages() {
            self.message(offset, message);
        }
    }

    /// Events the ring had no room for, wrapping at [`WRAP`].
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    fn push(&mut self, event: Outgoing) {
        let Some(ring) = &mut self.ring else { return };
        if ring.push(event).is_err() {
            self.dropped = (self.dropped + 1) % WRAP;
        }
    }
}

/// One cable sent as a controller: at each sample where its value, as
/// sent, changes, but no sooner than a gap after the last (#439). Two
/// changes inside the gap send only the later.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlLane {
    sent: Option<Control>,
    /// Samples still to wait before another value may go.
    wait: usize,
}

impl ControlLane {
    pub const fn new() -> Self {
        Self { sent: None, wait: 0 }
    }

    /// Runs the samples `samples` of a block (all of it, `0..frames`, or
    /// from where a note began): `control_at(i)` is what sample `i` would
    /// send, and it goes when it differs from what last went, at least
    /// `gap` samples after it. REAL-TIME SAFE.
    pub fn run(&mut self, writer: &mut MidiWriter, samples: Range<usize>, gap: usize, mut control_at: impl FnMut(usize) -> Control) {
        let Range { start: mut offset, end: frames } = samples;
        // The samples before the start count toward the wait
        self.wait = self.wait.saturating_sub(offset);
        while offset < frames {
            if self.wait > 0 {
                let skip = self.wait.min(frames - offset);
                offset += skip;
                self.wait -= skip;
                continue;
            }
            let control = control_at(offset);
            if self.sent != Some(control) {
                writer.control(offset, control);
                self.sent = Some(control);
                self.wait = gap.saturating_sub(1);
            }
            offset += 1;
        }
    }

    /// What it last sent.
    pub fn sent(&self) -> Option<Control> {
        self.sent
    }

    /// Forgets what it sent, so the next value goes whatever it is: for a
    /// cable unplugged, or a module moved to another port.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// The gap between a lane's values at `sample_rate`, in samples.
pub fn lane_gap(sample_rate: f32) -> usize {
    ((sample_rate * LANE_GAP).round() as usize).max(1)
}

/// Sends a Program Change (and its bank) as it changes, and again as a
/// trigger rises: the top of a song, say (#439).
#[derive(Clone, Copy, Debug)]
pub struct ProgramSlot {
    /// What went, and where: (port, channel, program).
    sent: Option<(usize, u8, Program)>,
    trigger: Edge,
}

impl Default for ProgramSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgramSlot {
    pub const fn new() -> Self {
        Self { sent: None, trigger: Edge::LOW }
    }

    /// Sends `program` (none chosen, nothing) at the top of the block if
    /// it, the port or the channel has changed since it last went, and at
    /// each sample `trigger` rises. Returns whether it sent: a device that
    /// takes up a program may take up its bend range too, so that is sent
    /// again after. REAL-TIME SAFE.
    pub fn run(&mut self, writer: &mut MidiWriter, port: usize, channel: u8, program: Option<Program>, trigger: Option<&[f32]>, frames: usize) -> bool {
        let Some(program) = program else {
            self.sent = None;
            if let Some(trigger) = trigger {
                trigger.iter().take(frames).for_each(|&x| {
                    self.trigger.tick(x);
                });
            }
            return false;
        };
        let mut sent = false;
        let place = Some((port, channel, program));
        if self.sent != place {
            writer.program(0, channel, program);
            self.sent = place;
            sent = true;
        }
        if let Some(trigger) = trigger {
            for (offset, &x) in trigger.iter().take(frames).enumerate() {
                if self.trigger.rising(x) {
                    writer.program(offset, channel, program);
                    sent = true;
                }
            }
        } else {
            self.trigger.reset();
        }
        sent
    }

    /// Forgets what went, so it goes again.
    pub fn reset(&mut self) {
        self.sent = None;
        self.trigger = Edge::LOW;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::midi::CcMode;
    use rtrb::Consumer;

    fn writer() -> (MidiWriter, Consumer<Outgoing>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let writer = MidiWriter::on_bus(bus);
        (writer, bus.take_new_streams().pop().unwrap())
    }

    fn controls(ring: &mut Consumer<Outgoing>) -> Vec<(u64, Control)> {
        let mut out = Vec::new();
        while let Ok(event) = ring.pop() {
            if let Outgoing::Control { sample, control } = event {
                out.push((sample, control));
            }
        }
        out
    }

    #[test]
    fn a_lane_sends_where_it_changes_no_faster_than_its_gap() {
        let (mut writer, mut ring) = writer();
        let mut lane = ControlLane::new();
        // A ramp across 64 samples: a new 7-bit value every sample or two
        let cc = |i: usize| Control::lane(CcMode::Bits7, 0, 74, 0, i as f32 / 63.0);
        lane.run(&mut writer, 0..64, 16, cc);
        let sent = controls(&mut ring);
        assert_eq!(sent.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [0, 16, 32, 48], "one per gap, where it changed");
        assert_eq!(sent[1].1, cc(16));
        // The gap carries over the block's end, and an unchanged value waits
        writer.end(64);
        lane.run(&mut writer, 0..64, 16, |_| cc(63));
        assert_eq!(controls(&mut ring), [(64, cc(63))], "the newest, as soon as the gap is up");
        writer.end(64);
        lane.run(&mut writer, 0..64, 16, |_| cc(63));
        assert!(controls(&mut ring).is_empty(), "unchanged");
        lane.reset();
        lane.run(&mut writer, 0..64, 16, |_| cc(63));
        assert_eq!(controls(&mut ring).len(), 1, "forgotten, so sent again");
    }

    #[test]
    fn a_program_goes_on_change_and_on_a_trigger() {
        let (mut writer, mut ring) = writer();
        let mut slot = ProgramSlot::new();
        let program = Program { bank_msb: Some(87), bank_lsb: Some(0), program: 4 };
        let low = [0.0; 8];
        slot.run(&mut writer, 0, 2, Some(program), Some(&low), 8);
        slot.run(&mut writer, 0, 2, Some(program), Some(&low), 8);
        let mut trigger = [0.0; 8];
        trigger[5] = 1.0;
        slot.run(&mut writer, 0, 2, Some(program), Some(&trigger), 8);
        slot.run(&mut writer, 1, 2, Some(program), None, 8);
        slot.run(&mut writer, 1, 2, None, None, 8);
        let mut sent = Vec::new();
        while let Ok(Outgoing::Message { sample, message }) = ring.pop() {
            sent.push((sample, message.bytes().to_vec()));
        }
        let burst = [vec![0xB2, 0, 87], vec![0xB2, 32, 0], vec![0xC2, 4]];
        let at = |sample: u64| burst.iter().map(move |m| (sample, m.clone()));
        let expected: Vec<_> = at(0).chain(at(5)).chain(at(0)).collect();
        assert_eq!(sent, expected, "once, again on the trigger, again on another port");
    }
}
