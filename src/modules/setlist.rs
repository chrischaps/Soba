//! Setlist: the Arranger tells the room (#445). At the chorus the Digitakt
//! moves to pattern A05, the FANTOM recalls Scene 12, the ZOIA loads patch 4
//! and the RC-505 recalls memory 7: each section has up to eight rows, each
//! a device's program, sent as Bank Select and Program Change.
//!
//! A row goes its **lead** ahead of the bar line, so a device that takes a
//! moment arrives on the downbeat: the Digitakt queues its next pattern for
//! the end of the one playing, so it hears it during the last bar. Others
//! switch at once, and go on the downbeat (a lead of 0). For that the
//! Setlist needs to know where the song goes before it gets there: the
//! Arranger's **Next** says, as soon as the last bar throws its dice or a
//! Jump is asked for, and its **Bar** measures how long the bar is. Without
//! Bar a row with a lead goes as soon as Next names its section; without
//! Next, every row goes on the downbeat its Section changes on.
//!
//! The rows go only while the transport plays. Starting, the section
//! coming first goes at once, before its downbeat: from the top, or from
//! wherever a Jump starts the song. **Send** sends the section playing
//! again, stopped or not.
//!
//! The rows reach the audio thread as [`RowMap`]s, by value, with
//! `EngineCommand::MapRow`, as an Instrument's knobs do. Each port has a
//! ring of its own, since a ring plays one port; a port no row uses is
//! left alone, so the Rig doesn't show it in use.

use crate::dsp::{
    context::ProcessContext,
    gate::Edge,
    module_trait::{DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer},
    SignalType,
};
use crate::hardware::setlist::{ROWS, SECTIONS};
use crate::hardware::RowMap;
use crate::link::midi::{MidiBus, MIDI, PORTS};
use crate::link::midi_writer::{MidiWriter, WRAP};

use super::arranger::Arranger;

/// The module's ID, as patches name it.
pub const ID: &str = "output.setlist";

/// Room in each port's ring: a whole section's rows many times over, and
/// the blocks between them while the sender is slow to drain it.
const RING: usize = 1024;

/// The longest a row's lamp counts, in seconds.
const LONGEST: f32 = 60.0;

/// Tells the hardware what each of a song's sections plays.
///
/// # Ports
///
/// **Inputs:**
/// - **Section** (Control): the Arranger's Section. The rows of the section
///   it changes to go on that downbeat, those that haven't already.
/// - **Next** (Control): the Arranger's Next. The rows of the section it
///   names go their lead ahead of the bar line.
/// - **Bar** (Gate): the Arranger's Bar, which measures the bar, so a row
///   knows when its lead begins.
/// - **Send** (Gate): sends the playing section's rows again as it rises.
///
/// # Rows
///
/// In the node's labels (see [`crate::hardware::setlist`]): each a port, a
/// channel, a bank and a program, and a lead in milliseconds.
pub struct Setlist {
    /// A writer to each port, A to H.
    writers: [MidiWriter; PORTS],
    cues: [[Option<RowMap>; ROWS]; SECTIONS],
    /// The ports a row goes out of.
    used: [bool; PORTS],
    bar: Edge,
    send: Edge,
    /// Samples since Bar rose, and the bar's length, between the last two.
    since_bar: Option<u64>,
    bar_length: Option<u64>,
    /// The transport played last sample.
    rolling: bool,
    /// The section Section read last sample.
    section: usize,
    /// The section every row of which has gone: what the room plays.
    ready: Option<usize>,
    /// The section going ahead of its bar line, and which of its rows have.
    ahead: Option<usize>,
    gone: [bool; ROWS],
    /// The section whose rows went last, and the sample each went on.
    shown: Option<usize>,
    lit: [Option<u64>; ROWS],
    /// Samples run so far.
    clock: u64,
    /// Rows sent, wrapping at [`WRAP`].
    sent: u32,
    sample_rate: f32,
    ports: Vec<PortDefinition>,
}

impl Setlist {
    pub const PORT_SECTION: usize = 0;
    pub const PORT_NEXT: usize = 1;
    pub const PORT_BAR: usize = 2;
    pub const PORT_SEND: usize = 3;

    /// 1 while a sender reads what it writes.
    pub const READOUT_LIVE: usize = 0;
    /// Rows sent, wrapping.
    pub const READOUT_SENT: usize = 1;
    /// The section the room plays, + 1, or 0.
    pub const READOUT_READY: usize = 2;
    /// The section going ahead of its bar line, + 1, or 0.
    pub const READOUT_AHEAD: usize = 3;
    /// Seconds to the bar line, or -1 while it isn't known.
    pub const READOUT_LEFT: usize = 4;
    /// The section whose rows went last, + 1, or 0.
    pub const READOUT_SHOWN: usize = 5;
    /// Seconds since each of its rows went, up to a minute.
    pub const READOUT_AGES: usize = 6;

    pub fn new() -> Self {
        Self::on_bus(&MIDI)
    }

    /// A Setlist writing to rings from `bus`: a test's own, say.
    pub fn on_bus(bus: &MidiBus) -> Self {
        let ports = vec![
            PortDefinition::input_with_default("section", "Section", SignalType::Control, 0.0)
                .describe("Patch the Arranger's Section: each section's rows go on its downbeat, those that haven't gone ahead"),
            PortDefinition::input_with_default("next", "Next", SignalType::Control, 0.0)
                .describe("Patch the Arranger's Next: the rows of the section coming go their lead ahead of the bar line"),
            PortDefinition::input_with_default("bar", "Bar", SignalType::Gate, 0.0)
                .describe("Patch the Arranger's Bar: it measures the bar, so a row's lead starts at the right moment"),
            PortDefinition::input_with_default("send", "Send", SignalType::Gate, 0.0).describe("Sends the playing section's rows again as it rises"),
        ];
        Self {
            writers: std::array::from_fn(|_| MidiWriter::on_bus_with(bus, RING)),
            cues: [[None; ROWS]; SECTIONS],
            used: [false; PORTS],
            bar: Edge::LOW,
            send: Edge::LOW,
            since_bar: None,
            bar_length: None,
            rolling: false,
            section: 0,
            ready: None,
            ahead: None,
            gone: [false; ROWS],
            shown: None,
            lit: [None; ROWS],
            clock: 0,
            sent: 0,
            sample_rate: 48_000.0,
            ports,
        }
    }

    /// Sends section `section`'s rows at `offset`, but those `skip` says
    /// have gone already.
    fn send_section(&mut self, section: usize, offset: usize, skip: [bool; ROWS]) {
        for row in 0..ROWS {
            if !skip[row] {
                self.go(section, row, offset);
            }
        }
    }

    /// Sends one row, if it has one, at `offset`. REAL-TIME SAFE.
    fn go(&mut self, section: usize, row: usize, offset: usize) {
        let Some(cue) = self.cues[section][row] else { return };
        self.writers[cue.port as usize].program(offset, cue.channel, cue.program);
        self.sent = (self.sent + 1) % WRAP;
        if self.shown != Some(section) {
            self.shown = Some(section);
            self.lit = [None; ROWS];
        }
        self.lit[row] = Some(self.clock + offset as u64);
    }

    /// Sends what's due of the section `coming`, ahead of a bar line
    /// `left` samples away (None when it's not known), at `offset`.
    fn ahead_of_the_bar(&mut self, coming: usize, left: Option<u64>, offset: usize) {
        if self.ahead != Some(coming) {
            self.ahead = Some(coming);
            self.gone = [false; ROWS];
        }
        for row in 0..ROWS {
            let Some(cue) = self.cues[coming][row] else {
                self.gone[row] = true;
                continue;
            };
            let lead = (cue.lead * self.sample_rate) as u64;
            if self.gone[row] || lead == 0 || left.is_some_and(|left| left > lead) {
                continue;
            }
            self.go(coming, row, offset);
            self.gone[row] = true;
            // The room is on its way to another section
            self.ready = None;
        }
        if self.gone.iter().all(|&gone| gone) {
            self.ready = Some(coming);
            self.ahead = None;
        }
    }

    /// The section a Section or Next CV names.
    fn section_of(cv: f32) -> usize {
        Arranger::section_for_cv(cv, SECTIONS)
    }
}

impl Default for Setlist {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Setlist {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Setlist",
            category: ModuleCategory::Output,
            description: "Each Arranger section changes the room: a device's pattern, scene, patch or memory, sent ahead of the bar",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &[]
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        // A bar measured at another rate is the wrong number of samples
        self.bar_length = None;
        self.since_bar = None;
    }

    fn process(&mut self, inputs: &[&SignalBuffer], _outputs: &mut [SignalBuffer], _params: &[f32], context: &ProcessContext) {
        let frames = context.block_size;
        // REAL-TIME SAFE: reading the clock and pushing to a ring
        for port in 0..PORTS {
            if self.used[port] {
                self.writers[port].begin(context, port);
            }
        }
        let transport = &context.transport;
        let playing = transport.beat_position.is_some() && transport.playing;
        let section_in = connected_input(inputs, Self::PORT_SECTION);
        let next_in = connected_input(inputs, Self::PORT_NEXT);
        let bar_in = connected_input(inputs, Self::PORT_BAR);
        let send_in = connected_input(inputs, Self::PORT_SEND);

        for i in 0..frames {
            let section = section_in.map_or(0, |cv| Self::section_of(cv.at(i)));
            let next = next_in.map(|cv| Self::section_of(cv.at(i)));
            if bar_in.is_some_and(|bar| self.bar.rising(bar.at(i))) {
                if let Some(since) = self.since_bar {
                    self.bar_length = Some(since);
                }
                self.since_bar = Some(0);
            }

            if playing && !self.rolling {
                // The top: the section coming first, before its downbeat
                let first = next.unwrap_or(section);
                self.send_section(first, i, [false; ROWS]);
                self.ready = Some(first);
                self.ahead = None;
                self.section = section;
            } else if playing {
                if section != self.section {
                    // The downbeat: whatever of the section hasn't gone ahead
                    self.section = section;
                    if self.ready != Some(section) {
                        let skip = if self.ahead == Some(section) { self.gone } else { [false; ROWS] };
                        self.send_section(section, i, skip);
                        self.ready = Some(section);
                    }
                    self.ahead = None;
                }
                if let Some(coming) = next.filter(|&coming| Some(coming) != self.ready) {
                    let left = self.bar_length.zip(self.since_bar).map(|(length, since)| length.saturating_sub(since));
                    self.ahead_of_the_bar(coming, left, i);
                }
            } else if self.rolling {
                // Stopped: the next start sends afresh
                self.ready = None;
                self.ahead = None;
            }
            self.rolling = playing;

            if send_in.is_some_and(|send| self.send.rising(send.at(i))) {
                self.send_section(section, i, [false; ROWS]);
            }
            if let Some(since) = self.since_bar.as_mut() {
                *since += 1;
            }
        }

        self.clock += frames as u64;
        for writer in &mut self.writers {
            writer.end(frames);
        }
    }

    fn reset(&mut self) {
        self.rolling = false;
        self.ready = None;
        self.ahead = None;
        self.bar = Edge::LOW;
        self.send = Edge::LOW;
        self.since_bar = None;
    }

    fn map_row(&mut self, section: usize, row: usize, cue: Option<RowMap>) {
        if section >= SECTIONS || row >= ROWS {
            return;
        }
        self.cues[section][row] = cue;
        self.used = [false; PORTS];
        for cue in self.cues.iter().flatten().flatten() {
            self.used[cue.port as usize % PORTS] = true;
        }
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        let values = &mut readout.values;
        let plus_one = |section: Option<usize>| section.map_or(0.0, |s| (s + 1) as f32);
        values[Self::READOUT_LIVE] = if self.writers[0].is_live() { 1.0 } else { 0.0 };
        values[Self::READOUT_SENT] = self.sent as f32;
        values[Self::READOUT_READY] = plus_one(self.ready);
        values[Self::READOUT_AHEAD] = plus_one(self.ahead);
        values[Self::READOUT_LEFT] = match self.bar_length.zip(self.since_bar) {
            Some((length, since)) if self.rolling => length.saturating_sub(since) as f32 / self.sample_rate,
            _ => -1.0,
        };
        values[Self::READOUT_SHOWN] = plus_one(self.shown);
        for (row, lit) in self.lit.iter().enumerate() {
            values[Self::READOUT_AGES + row] = lit.map_or(LONGEST, |at| (self.clock.saturating_sub(at) as f32 / self.sample_rate).min(LONGEST));
        }
        Some(readout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::context::TransportState;
    use crate::link::midi::{Outgoing, Program};
    use rtrb::Consumer;

    const RATE: f32 = 1000.0;

    /// A Clock's transport, playing or stopped.
    fn transport(playing: bool) -> TransportState {
        TransportState { playing, beat_position: Some(0.0), ..TransportState::playing_at(150.0) }
    }

    /// A Setlist on a bus of its own, and the far end of each port's ring.
    fn module() -> (Setlist, Vec<Consumer<Outgoing>>) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        bus.start_draining();
        let mut module = Setlist::on_bus(bus);
        module.prepare(RATE, 1024);
        (module, bus.take_new_streams())
    }

    fn row_map(port: u8, channel: u8, program: u8, lead_ms: u16) -> Option<RowMap> {
        Some(RowMap { port, channel, program: Program { bank_msb: None, bank_lsb: None, program }, lead: lead_ms as f32 / 1000.0 })
    }

    /// Every message sent on any port, as (port, sample, bytes), in time order.
    fn sent(rings: &mut [Consumer<Outgoing>]) -> Vec<(usize, u64, Vec<u8>)> {
        let mut out = Vec::new();
        for (port, ring) in rings.iter_mut().enumerate() {
            while let Ok(event) = ring.pop() {
                if let Outgoing::Message { sample, message } = event {
                    out.push((port, sample, message.bytes().to_vec()));
                }
            }
        }
        out.sort_by_key(|(port, sample, _)| (*sample, *port));
        out
    }

    /// The program changes in `sent`, as (port, sample, channel from 0, program).
    fn programs(sent: &[(usize, u64, Vec<u8>)]) -> Vec<(usize, u64, u8, u8)> {
        sent.iter().filter(|(_, _, m)| m[0] & 0xF0 == 0xC0).map(|(port, sample, m)| (*port, *sample, m[0] & 0x0F, m[1])).collect()
    }

    /// An Arranger's song as the Setlist hears it, a sample a millisecond:
    /// `sections` (each a section and its bars), four 100 ms beats to the
    /// bar, with Next turning in each section's last bar, as the Arranger's
    /// does.
    struct Song {
        section: Vec<f32>,
        next: Vec<f32>,
        bar: Vec<f32>,
    }

    const BAR: usize = 400;

    fn song(sections: &[(usize, usize)]) -> Song {
        let cv = |s: usize| s as f32 / SECTIONS as f32;
        let (mut section, mut next, mut bar) = (Vec::new(), Vec::new(), Vec::new());
        for (index, &(playing, bars)) in sections.iter().enumerate() {
            let coming = sections.get(index + 1).map_or(playing, |&(s, _)| s);
            for b in 0..bars {
                for t in 0..BAR {
                    section.push(cv(playing));
                    next.push(cv(if b + 1 == bars { coming } else { playing }));
                    bar.push(if t < 5 { 1.0 } else { 0.0 });
                }
            }
        }
        Song { section, next, bar }
    }

    /// Plays `song` into the Setlist in 64-sample blocks, the transport
    /// playing from `start`, with Next and Bar patched if asked.
    fn play(module: &mut Setlist, song: &Song, start: usize, patched: (bool, bool)) {
        const BLOCK: usize = 64;
        let mut at = 0;
        while at < song.section.len() {
            let n = BLOCK.min(song.section.len() - at);
            let buffer = |from: &[f32], signal: SignalType| {
                let mut buffer = if signal == SignalType::Gate { SignalBuffer::gate(n) } else { SignalBuffer::control(n) };
                buffer.samples[..n].copy_from_slice(&from[at..at + n]);
                buffer
            };
            let section = buffer(&song.section, SignalType::Control);
            let next = if patched.0 { buffer(&song.next, SignalType::Control) } else { SignalBuffer::unconnected(n, SignalType::Control) };
            let bar = if patched.1 { buffer(&song.bar, SignalType::Gate) } else { SignalBuffer::unconnected(n, SignalType::Gate) };
            let send = SignalBuffer::unconnected(n, SignalType::Gate);
            let transport = transport(at >= start);
            module.process(&[&section, &next, &bar, &send], &mut [], &[], &ProcessContext::with_transport(RATE, n, transport));
            at += n;
        }
    }

    #[test]
    fn ports_and_info() {
        let (module, rings) = module();
        assert_eq!(module.info().id, "output.setlist");
        assert_eq!(rings.len(), PORTS, "a ring a port");
        let names: Vec<&str> = module.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["Section", "Next", "Bar", "Send"]);
        assert!(module.ports().iter().all(|p| p.is_input()));
        assert!(module.parameters().is_empty());
    }

    #[test]
    fn each_section_change_sends_its_rows_their_lead_before_the_bar() {
        let (mut module, mut rings) = module();
        // Verse: the Digitakt (port A, ch 1) to pattern 0. Chorus: pattern
        // 4 a quarter bar ahead, and the FANTOM (port B, ch 3) on the
        // downbeat, after its bank
        module.map_row(0, 0, row_map(0, 0, 0, 0));
        module.map_row(1, 0, row_map(0, 0, 4, 100));
        let fantom = RowMap { port: 1, channel: 2, program: Program { bank_msb: Some(85), bank_lsb: Some(1), program: 11 }, lead: 0.0 };
        module.map_row(1, 3, Some(fantom));
        play(&mut module, &song(&[(0, 2), (1, 2), (0, 1)]), 0, (true, true));
        let sent = sent(&mut rings);
        let chorus = 2 * BAR as u64;
        assert_eq!(
            programs(&sent),
            [(0, 0, 0, 0), (0, chorus - 100, 0, 4), (1, chorus, 2, 11), (0, 4 * BAR as u64, 0, 0)],
            "the verse at the top; the chorus's Digitakt 100 ms early, its FANTOM on the downbeat; the verse again with no lead"
        );
        let fantom_burst: Vec<_> = sent.iter().filter(|(port, _, _)| *port == 1).map(|(_, s, m)| (*s, m.clone())).collect();
        assert_eq!(fantom_burst, [(chorus, vec![0xB2, 0, 85]), (chorus, vec![0xB2, 32, 1]), (chorus, vec![0xC2, 11])], "the bank, then the program");
    }

    #[test]
    fn starting_mid_song_sends_the_coming_section_before_its_downbeat() {
        let (mut module, mut rings) = module();
        module.map_row(0, 0, row_map(0, 0, 0, 0));
        module.map_row(2, 0, row_map(0, 0, 9, 0));
        module.map_row(2, 1, row_map(3, 5, 7, 250));
        // A Jump to section 3 before the first clock: Section still reads
        // 1 while Next names 3, then the first clock starts 3
        let mut song = song(&[(2, 2)]);
        let first = SECTIONS as f32;
        for t in 0..10 {
            song.section[t] = 0.0;
            song.bar[t] = 0.0;
        }
        song.next[..10].fill(2.0 / first);
        play(&mut module, &song, 0, (true, true));
        assert_eq!(programs(&sent(&mut rings)), [(0, 0, 0, 9), (3, 0, 5, 7)], "section 3's rows at once, and not again on its downbeat");
    }

    #[test]
    fn a_stopped_transport_sends_nothing_and_starting_sends_the_section() {
        let (mut module, mut rings) = module();
        module.map_row(0, 0, row_map(0, 0, 1, 0));
        module.map_row(1, 0, row_map(0, 0, 2, 0));
        play(&mut module, &song(&[(0, 1), (1, 1)]), usize::MAX, (true, true));
        assert!(sent(&mut rings).is_empty(), "stopped");
        let (mut module, mut rings) = self::module();
        module.map_row(0, 0, row_map(0, 0, 1, 0));
        module.map_row(1, 0, row_map(0, 0, 2, 0));
        play(&mut module, &song(&[(0, 1), (1, 1)]), 500, (true, true));
        assert_eq!(programs(&sent(&mut rings)), [(0, 512, 0, 2)], "started in section 2: its rows as the block it starts in begins");
    }

    #[test]
    fn without_next_rows_go_on_the_downbeat_and_without_bar_as_soon_as_known() {
        let (mut module, mut rings) = module();
        module.map_row(1, 0, row_map(0, 0, 4, 100));
        play(&mut module, &song(&[(0, 2), (1, 1)]), 0, (false, true));
        assert_eq!(programs(&sent(&mut rings)), [(0, 800, 0, 4)], "no Next: on the downbeat, lead or not");
        let (mut module, mut rings) = self::module();
        module.map_row(1, 0, row_map(0, 0, 4, 100));
        play(&mut module, &song(&[(0, 2), (1, 1)]), 0, (true, false));
        assert_eq!(programs(&sent(&mut rings)), [(0, 400, 0, 4)], "no Bar: as soon as Next names it, a bar ahead");
    }

    #[test]
    fn a_change_of_plan_sends_the_section_the_song_goes_to_after_all() {
        let (mut module, mut rings) = module();
        module.map_row(0, 0, row_map(0, 0, 0, 200));
        module.map_row(1, 0, row_map(0, 0, 1, 200));
        // In section 1's second bar, Next names 2, and then a Jump back to 1
        // 150 ms before the bar line, after 2's row has gone ahead
        let mut song = song(&[(0, 2), (1, 1)]);
        song.next[BAR..2 * BAR].fill(1.0 / SECTIONS as f32);
        song.next[2 * BAR - 150..2 * BAR].fill(0.0);
        song.section[2 * BAR..].fill(0.0);
        song.next[2 * BAR..].fill(0.0);
        play(&mut module, &song, 0, (true, true));
        let sent = programs(&sent(&mut rings));
        assert_eq!(sent, [(0, 0, 0, 0), (0, 600, 0, 1), (0, 650, 0, 0)], "the room goes back to section 1 at once");
    }

    #[test]
    fn send_sends_the_playing_section_again_and_the_readout_lights_its_rows() {
        let (mut module, mut rings) = module();
        module.map_row(0, 2, row_map(5, 0, 3, 0));
        let section = SignalBuffer::control(16);
        let mut send = SignalBuffer::gate(16);
        send.samples[7] = 1.0;
        let none = SignalBuffer::unconnected(16, SignalType::Control);
        let context = ProcessContext::with_transport(RATE, 16, transport(false));
        module.process(&[&section, &none, &none, &send], &mut [], &[], &context);
        assert_eq!(programs(&sent(&mut rings)), [(5, 7, 0, 3)], "stopped or not");
        let values = module.readout(&[]).unwrap().values;
        assert_eq!(values[Setlist::READOUT_SHOWN], 1.0);
        assert!((values[Setlist::READOUT_AGES + 2] - 0.009).abs() < 1e-6, "lit 9 ms ago");
        assert_eq!(values[Setlist::READOUT_AGES], LONGEST, "a row with nothing in it");
        assert_eq!(values[Setlist::READOUT_SENT], 1.0);
    }

    #[test]
    fn only_the_ports_its_rows_use_are_played() {
        let (mut module, mut rings) = module();
        module.map_row(4, 0, row_map(6, 0, 0, 0));
        let none = SignalBuffer::unconnected(8, SignalType::Control);
        module.process(&[&none, &none, &none, &none], &mut [], &[], &ProcessContext::new(RATE, 8));
        for (port, ring) in rings.iter_mut().enumerate() {
            let blocks = std::iter::from_fn(|| ring.pop().ok()).filter(|e| matches!(e, Outgoing::Block { .. })).count();
            assert_eq!(blocks, usize::from(port == 6), "port {port}");
        }
        // Taken away, the port is left alone
        module.map_row(4, 0, None);
        assert!(!module.used.iter().any(|&used| used));
    }
}
