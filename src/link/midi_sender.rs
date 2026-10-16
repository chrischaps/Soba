//! The MIDI half of the link's sender thread: drains every MIDI Out's ring
//! and sends each message to its port's device when its sample is heard.
//!
//! **When a sample is heard.** Each block a MIDI Out says which sample it
//! starts at and when it leaves for the output device. An audio callback
//! never runs early, only late (the system was busy), so the earliest
//! `sent − sample / rate` seen is the best guess at when sample 0 left. It
//! is let creep later by a little each block, to follow the sound card's
//! clock as it drifts from the computer's. A sample is heard that long
//! after sample 0 left, plus the output device's delay.
//!
//! **Stuck notes.** The sender watches every Note On and Note Off it
//! sends, per module. When a module's ring is dropped (the module deleted,
//! or another patch loaded) or it stops running (the audio stopped), the
//! notes it still holds are let go, with an All Notes Off on their
//! channels. A port moved to another device lets go of its notes on the
//! old one first.

use std::collections::VecDeque;
use std::time::Duration;

use rtrb::Consumer;
use web_time::Instant;

use super::midi::{Message, MidiBus, Outgoing, PortStatus, ALL_NOTES_OFF, PORTS};

/// How often messages are looked at while a MIDI Out runs. MIDI clock ticks
/// every 20 ms at 125 BPM, so a millisecond keeps a following DAW steady.
pub const TICK: Duration = Duration::from_millis(1);
/// How often the rings are looked at while MIDI Outs are in the patch but
/// none runs (the audio is stopped): soon enough that the first notes
/// after it starts aren't late.
const WAITING_TICK: Duration = Duration::from_millis(5);
/// How often the rings are looked at while there's no MIDI Out at all.
const IDLE_TICK: Duration = Duration::from_millis(50);
/// A module that hasn't run a block for this long has stopped: the stream
/// paused, or the device was lost. Its notes are let go.
const STALE: Duration = Duration::from_millis(250);
/// How often the devices are listed again, for one plugged in later.
const SCAN_EVERY: Duration = Duration::from_secs(2);
/// How long a device that wouldn't open is left before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(2);
/// How far the guess at sample 0 creeps later each block, at most: 2 µs a
/// 10 ms block is 200 ppm, more than any sound card drifts from the
/// computer's clock, and small enough that callbacks running a little late
/// don't drag the timing late with them.
const CREEP: Duration = Duration::from_micros(2);

/// Asks the system to run the calling thread ahead of ordinary work, as
/// audio threads are: MIDI clock that waits behind a busy UI or a build
/// arrives late, and a following DAW hears the tempo wobble.
pub fn raise_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
        // SAFETY: the pseudo-handle for this thread is always valid
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
    }
}

/// The machine's MIDI outputs: midir on the desktop, a recorder in tests.
pub trait Outputs: Send {
    /// The names of the outputs there are now.
    fn scan(&mut self) -> Vec<String>;
    /// Opens the output called `name`.
    fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String>;
}

/// An open output.
pub trait Connection: Send {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// Sends a bus's MIDI.
pub struct MidiSender {
    bus: &'static MidiBus,
    outputs: Box<dyn Outputs>,
    streams: Vec<Stream>,
    ports: [Port; PORTS],
    devices: Vec<String>,
    scanned_at: Option<Instant>,
}

/// One port's device, as opened.
#[derive(Default)]
struct Port {
    /// The device it plays, as last applied from the bus.
    device: String,
    connection: Option<Box<dyn Connection>>,
    failed_at: Option<Instant>,
}

/// One MIDI Out's ring, and what sending it remembers.
struct Stream {
    events: Consumer<Outgoing>,
    port: usize,
    rate: f32,
    /// When sample 0 was due, as best known.
    origin: Option<Instant>,
    last_block: Option<Instant>,
    /// Messages waiting for their sample, in the order they're due.
    pending: VecDeque<(u64, Message)>,
    /// The notes it holds, a bit per note, per channel.
    held: [u128; 16],
}

impl Stream {
    fn new(events: Consumer<Outgoing>) -> Self {
        Self { events, port: 0, rate: 48_000.0, origin: None, last_block: None, pending: VecDeque::new(), held: [0; 16] }
    }

    /// When sample `sample` is heard, if a block has said yet.
    fn due(&self, sample: u64, latency: Duration) -> Option<Instant> {
        let origin = self.origin?;
        Some(origin + Duration::from_secs_f64(sample as f64 / self.rate as f64) + latency)
    }

    /// Learns from a block that `sample` was being rendered at `at`.
    fn learn(&mut self, sample: u64, at: Instant, rate: f32) {
        self.rate = rate.max(1.0);
        let offset = Duration::from_secs_f64(sample as f64 / self.rate as f64);
        let Some(origin) = at.checked_sub(offset) else { return };
        self.origin = Some(match self.origin {
            Some(known) if origin >= known => known + (origin - known).min(CREEP),
            _ => origin,
        });
    }

    /// Keeps `message` in time order: after everything due at or before
    /// its sample, so a block's own order stands.
    fn queue(&mut self, sample: u64, message: Message) {
        let at = self.pending.partition_point(|&(s, _)| s <= sample);
        self.pending.insert(at, (sample, message));
    }

    fn holds_notes(&self) -> bool {
        self.held.iter().any(|&notes| notes != 0)
    }
}

impl MidiSender {
    /// A sender for `bus`, playing to `outputs`. The bus hands out rings from now on.
    pub fn new(bus: &'static MidiBus, outputs: Box<dyn Outputs>) -> Self {
        bus.start_draining();
        Self { bus, outputs, streams: Vec::new(), ports: Default::default(), devices: Vec::new(), scanned_at: None }
    }

    /// Sends whatever is due, and says when to look again.
    pub fn tick(&mut self, now: Instant) -> Instant {
        self.streams.extend(self.bus.take_new_streams().into_iter().map(Stream::new));
        if self.bus.take_rescan() || self.scanned_at.is_none_or(|at| now.duration_since(at) >= SCAN_EVERY) {
            self.scanned_at = Some(now);
            self.devices = self.outputs.scan();
            self.bus.set_devices(self.devices.clone());
        }
        self.follow_choices();

        let latency = self.bus.latency();
        let mut busy = false;
        let mut index = 0;
        while index < self.streams.len() {
            let gone = self.drain(index, now);
            let stream = &mut self.streams[index];
            let stale = stream.last_block.is_none_or(|at| now.duration_since(at) >= STALE);
            // Everything due goes out; a module that's gone or stopped
            // sends what it left at once, then lets go of its notes
            let flush = gone || stale;
            while let Some(&(sample, message)) = stream.pending.front() {
                if !flush && stream.due(sample, latency).is_none_or(|due| due > now) {
                    break;
                }
                stream.pending.pop_front();
                let port = stream.port;
                Self::send(&mut self.ports[port], &mut self.outputs, &self.devices, &mut stream.held, message, now);
            }
            if flush && stream.holds_notes() {
                let port = stream.port;
                Self::release(&mut self.ports[port], &mut self.outputs, &self.devices, &mut stream.held, now);
            }
            if gone {
                self.streams.swap_remove(index);
                continue;
            }
            busy |= !stale;
            index += 1;
        }
        self.report();
        now + if busy {
            TICK
        } else if !self.streams.is_empty() {
            WAITING_TICK
        } else {
            IDLE_TICK
        }
    }

    /// Takes stream `index`'s new events. Returns whether its module is gone.
    fn drain(&mut self, index: usize, now: Instant) -> bool {
        loop {
            let stream = &mut self.streams[index];
            let Ok(event) = stream.events.pop() else { break };
            match event {
                Outgoing::Block { sample, at, rate, port } => {
                    let port = (port as usize).min(PORTS - 1);
                    if port != stream.port && stream.holds_notes() {
                        // Moved to another port: its notes stop where they play
                        let old = stream.port;
                        Self::release(&mut self.ports[old], &mut self.outputs, &self.devices, &mut stream.held, now);
                    }
                    stream.port = port;
                    stream.learn(sample, at, rate);
                    stream.last_block = Some(now);
                }
                Outgoing::Message { sample, message } => stream.queue(sample, message),
            }
        }
        self.streams[index].events.is_abandoned() && self.streams[index].events.is_empty()
    }

    /// Takes up each port's device as the settings now say, letting go of
    /// the notes on the one it leaves.
    fn follow_choices(&mut self) {
        let chosen = self.bus.all_chosen();
        for (index, device) in chosen.into_iter().enumerate() {
            if self.ports[index].device == device {
                continue;
            }
            let now = Instant::now();
            for stream in self.streams.iter_mut().filter(|s| s.port == index) {
                Self::release(&mut self.ports[index], &mut self.outputs, &self.devices, &mut stream.held, now);
            }
            self.ports[index] = Port { device, ..Port::default() };
        }
    }

    /// Sends `message` on `port`, opening its device first if need be, and
    /// keeps `held` up to date.
    fn send(port: &mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], held: &mut [u128; 16], message: Message, now: Instant) {
        if let Some((channel, note)) = message.starts() {
            held[channel as usize] |= 1 << note;
        } else if let Some((channel, note)) = message.ends() {
            held[channel as usize] &= !(1 << note);
        }
        if let Some(connection) = Self::connection(port, outputs, devices, now) {
            if let Err(error) = connection.send(message.bytes()) {
                eprintln!("MIDI Out: sending to {} failed: {error}", port.device);
                port.connection = None;
                port.failed_at = Some(now);
            }
        }
    }

    /// Lets go of every note in `held` on `port`, with All Notes Off on
    /// their channels.
    fn release(port: &mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], held: &mut [u128; 16], now: Instant) {
        for channel in 0..16u8 {
            let notes = std::mem::take(&mut held[channel as usize]);
            if notes == 0 {
                continue;
            }
            let mut ignored = [0u128; 16];
            for note in (0..128u8).filter(|&n| notes & (1 << n) != 0) {
                Self::send(port, outputs, devices, &mut ignored, Message::note_off(channel, note), now);
            }
            Self::send(port, outputs, devices, &mut ignored, Message::control(channel, ALL_NOTES_OFF, 0), now);
        }
    }

    /// `port`'s open device, opening it if it's plugged in and hasn't just
    /// refused.
    fn connection<'a>(port: &'a mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], now: Instant) -> Option<&'a mut Box<dyn Connection>> {
        if port.connection.is_none() && !port.device.is_empty() && devices.contains(&port.device) {
            if port.failed_at.is_none_or(|at| now.duration_since(at) >= RETRY_AFTER) {
                match outputs.open(&port.device) {
                    Ok(connection) => {
                        port.connection = Some(connection);
                        port.failed_at = None;
                    }
                    Err(error) => {
                        eprintln!("MIDI Out: couldn't open {}: {error}", port.device);
                        port.failed_at = Some(now);
                    }
                }
            }
        }
        port.connection.as_mut()
    }

    /// Tells each MIDI Out's face how its port is doing.
    fn report(&mut self) {
        for (index, port) in self.ports.iter_mut().enumerate() {
            let used = self.streams.iter().any(|s| s.port == index && s.last_block.is_some());
            let status = if !used {
                PortStatus::Idle
            } else if port.device.is_empty() {
                PortStatus::NoDevice
            } else if !self.devices.contains(&port.device) {
                // Unplugged: open it afresh when it comes back
                port.connection = None;
                PortStatus::Missing
            } else if port.failed_at.is_some() {
                PortStatus::Failed
            } else {
                PortStatus::Sending
            };
            self.bus.set_status(index, status);
        }
    }
}

/// The machine's MIDI outputs, through midir.
pub struct Midir;

impl Outputs for Midir {
    fn scan(&mut self) -> Vec<String> {
        let Ok(output) = midir::MidiOutput::new("Soba Scanner") else { return Vec::new() };
        output.ports().iter().filter_map(|port| output.port_name(port).ok()).collect()
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String> {
        let output = midir::MidiOutput::new("Soba").map_err(|e| e.to_string())?;
        let port = output
            .ports()
            .into_iter()
            .find(|port| output.port_name(port).is_ok_and(|n| n == name))
            .ok_or_else(|| "not plugged in".to_string())?;
        let connection = output
            .connect(&port, "Soba MIDI Out")
            // Windows lets one app at a time open a port
            .map_err(|e| format!("{e} (is another app using it?)"))?;
        Ok(Box::new(connection))
    }
}

impl Connection for midir::MidiOutputConnection {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        midir::MidiOutputConnection::send(self, bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::midi::status;
    use std::sync::{Arc, Mutex};

    /// What a test's outputs were sent: (device, bytes, when).
    type Log = Arc<Mutex<Vec<(String, Vec<u8>, Instant)>>>;

    struct Recorder {
        devices: Vec<String>,
        log: Log,
        refuse: bool,
    }

    struct Recording {
        name: String,
        log: Log,
    }

    impl Outputs for Recorder {
        fn scan(&mut self) -> Vec<String> {
            self.devices.clone()
        }
        fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String> {
            if self.refuse {
                return Err("in use".into());
            }
            Ok(Box::new(Recording { name: name.into(), log: self.log.clone() }))
        }
    }

    impl Connection for Recording {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.log.lock().unwrap().push((self.name.clone(), bytes.to_vec(), Instant::now()));
            Ok(())
        }
    }

    fn setup(refuse: bool) -> (&'static MidiBus, MidiSender, Log) {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        let log = Log::default();
        let outputs = Recorder { devices: vec!["Synth".into(), "DAW".into()], log: log.clone(), refuse };
        let sender = MidiSender::new(bus, Box::new(outputs));
        bus.choose(0, Some("Synth"));
        (bus, sender, log)
    }

    fn sent(log: &Log) -> Vec<Vec<u8>> {
        log.lock().unwrap().iter().map(|(_, bytes, _)| bytes.clone()).collect()
    }

    const RATE: f32 = 48_000.0;

    #[test]
    fn messages_wait_for_their_sample() {
        let (bus, mut sender, log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        // Half a second in
        ring.push(Outgoing::Message { sample: 24_000, message: Message::note_on(0, 60, 100) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::real_time(status::START) }).unwrap();
        assert_eq!(sender.tick(start), start + TICK, "busy");
        assert_eq!(sent(&log), vec![vec![status::START]], "only what's due, in time order");
        ring.push(Outgoing::Block { sample: 19_200, at: start + Duration::from_millis(400), rate: RATE, port: 0 }).unwrap();
        sender.tick(start + Duration::from_millis(499));
        assert_eq!(sent(&log).len(), 1);
        ring.push(Outgoing::Block { sample: 24_000, at: start + Duration::from_millis(500), rate: RATE, port: 0 }).unwrap();
        sender.tick(start + Duration::from_millis(500));
        assert_eq!(sent(&log)[1], vec![0x90, 60, 100]);
        assert_eq!(bus.status(0), PortStatus::Sending);
    }

    #[test]
    fn the_output_delay_holds_messages_back() {
        let (bus, mut sender, log) = setup(false);
        bus.set_latency(Some(Duration::from_millis(30)));
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 60, 100) }).unwrap();
        sender.tick(start + Duration::from_millis(29));
        assert!(sent(&log).is_empty());
        sender.tick(start + Duration::from_millis(30));
        assert_eq!(sent(&log).len(), 1);
    }

    #[test]
    fn a_late_block_doesnt_move_the_timing() {
        let mut stream = Stream::new(RingBuffer::<Outgoing>::new(4).1);
        let start = Instant::now();
        stream.learn(0, start, RATE);
        // The next 10 ms block's callback ran 3 ms late
        stream.learn(480, start + Duration::from_millis(13), RATE);
        assert_eq!(stream.origin.unwrap(), start + CREEP, "creeps, doesn't jump");
        // An early one is taken at once
        stream.learn(960, start + Duration::from_millis(15), RATE);
        assert_eq!(stream.origin.unwrap(), start - Duration::from_millis(5));
    }

    use rtrb::RingBuffer;

    #[test]
    fn a_deleted_module_lets_go_of_its_notes() {
        let (bus, mut sender, log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(3, 60, 90) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(3, 64, 90) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_off(3, 60) }).unwrap();
        sender.tick(start);
        assert_eq!(sent(&log).len(), 3);
        drop(ring);
        sender.tick(start + TICK);
        assert_eq!(&sent(&log)[3..], &[vec![0x83, 64, 0], vec![0xB3, ALL_NOTES_OFF, 0]], "only the note still held");
        assert!(sender.streams.is_empty());
        sender.tick(start + TICK * 2);
        assert_eq!(sent(&log).len(), 5, "once");
    }

    #[test]
    fn a_module_that_stops_running_lets_go() {
        let (bus, mut sender, log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 48, 90) }).unwrap();
        sender.tick(start);
        sender.tick(start + STALE / 2);
        assert_eq!(sent(&log).len(), 1, "a slow block is no reason to stop");
        assert_eq!(sender.tick(start + STALE), start + STALE + WAITING_TICK, "waiting");
        assert_eq!(sent(&log)[1], vec![0x80, 48, 0]);
        assert_eq!(bus.status(0), PortStatus::Sending, "still on its port");
    }

    #[test]
    fn a_new_device_takes_over_after_the_old_lets_go() {
        let (bus, mut sender, log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 50, 90) }).unwrap();
        sender.tick(start);
        bus.choose(0, Some("DAW"));
        ring.push(Outgoing::Block { sample: 48, at: start + TICK, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 48, message: Message::note_on(0, 52, 90) }).unwrap();
        sender.tick(start + TICK);
        let log = log.lock().unwrap();
        let to: Vec<(&str, &[u8])> = log.iter().map(|(d, b, _)| (d.as_str(), b.as_slice())).collect();
        assert_eq!(
            to,
            [("Synth", &[0x90, 50, 90][..]), ("Synth", &[0x80, 50, 0]), ("Synth", &[0xB0, ALL_NOTES_OFF, 0]), ("DAW", &[0x90, 52, 90])]
        );
    }

    #[test]
    fn ports_say_what_they_lack() {
        let (bus, mut sender, _log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        sender.tick(start);
        assert_eq!(bus.status(1), PortStatus::Idle, "no MIDI Out on it");
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 1 }).unwrap();
        sender.tick(start);
        assert_eq!(bus.status(1), PortStatus::NoDevice);
        bus.choose(1, Some("Unplugged"));
        sender.tick(start);
        assert_eq!(bus.status(1), PortStatus::Missing);
        assert_eq!(bus.devices(), ["Synth", "DAW"]);
    }

    #[test]
    fn a_device_in_use_says_so() {
        let (bus, mut sender, log) = setup(true);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 60, 90) }).unwrap();
        sender.tick(start);
        assert_eq!(bus.status(0), PortStatus::Failed);
        assert!(sent(&log).is_empty());
    }
}
