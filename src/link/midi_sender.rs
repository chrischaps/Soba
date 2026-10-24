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
//!
//! **Renamed devices.** A port looks for its device by name, and if the
//! system has renumbered it ("2- Digitakt" is now "3- Digitakt"), by the
//! name without the number (see [`device_names`]), so the rig comes back
//! after a reboot plugs it in a different order.
//!
//! **The patch's clock** (#437) comes from the engine on a ring of its own
//! ([`midi_clock`](super::midi_clock)), and each port takes what its
//! [`ClockMode`] wants of it, however many MIDI Outs play the port, or
//! none. A port that starts sending the transport while the song plays is
//! brought in on the next sixteenth, with the place and Continue; one that
//! stops is sent Stop.
//!
//! **Offsets.** Each port's notes, CCs and clock go out its offset later
//! than the audio beside them, or earlier, but only by as much as the
//! output's delay: a message can't go before its block is made.
//!
//! **Ahead of time.** The desktop's thread wakes every millisecond and
//! sends each message as it falls due. The browser has no thread, only a
//! timer that a busy frame can hold up, so there the sender looks
//! [`MidiSender::ahead`] and hands each message over early with the moment
//! it's due, which Web MIDI keeps to (`web_sender`). Notes it lets go are
//! stamped after the last message it scheduled on their port, so a Note On
//! already on its way can't land after the All Notes Off.
//!
//! **Thru** (#446). What a MIDI In passes down a route
//! ([`midi_thru`](super::midi_thru)) wakes the thread and goes out at once,
//! without the output's delay or the port's offset ([`MidiSender::forward`]).

use std::collections::VecDeque;
use std::time::Duration;

use rtrb::Consumer;
use web_time::Instant;

use super::midi::{status, ClockMode, Message, MidiBus, Outgoing, PortStatus, Traffic, ALL_NOTES_OFF, PORTS};
use crate::engine::device_names;

mod thru;

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

/// `Send` on the desktop, where the sender has a thread of its own. The
/// browser has one thread, and its Web MIDI handles aren't `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait ThreadSafe: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> ThreadSafe for T {}
#[cfg(target_arch = "wasm32")]
pub trait ThreadSafe {}
#[cfg(target_arch = "wasm32")]
impl<T> ThreadSafe for T {}

/// The machine's MIDI outputs: midir on the desktop, Web MIDI in the
/// browser, a recorder in tests.
pub trait Outputs: ThreadSafe {
    /// The names of the outputs there are now.
    fn scan(&mut self) -> Vec<String>;
    /// Opens the output called `name`.
    fn open(&mut self, name: &str) -> Result<Box<dyn Connection>, String>;
}

/// An open output.
pub trait Connection: ThreadSafe {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// Sends `bytes` to play at `at`, which may be a little ahead. An
    /// output that can't wait sends them now.
    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), String> {
        let _ = at;
        self.send(bytes)
    }
}

/// Sends a bus's MIDI.
pub struct MidiSender {
    bus: &'static MidiBus,
    outputs: Box<dyn Outputs>,
    streams: Vec<Stream>,
    /// The patch's clock, once the engine has made its ring.
    clock: Option<ClockStream>,
    ports: [Port; PORTS],
    devices: Vec<String>,
    scanned_at: Option<Instant>,
    /// How far ahead of its moment a message is sent: none on the desktop.
    ahead: Duration,
    /// What MIDI Ins pass straight through (#446).
    thru: thru::Thru,
}

/// One port's device, as opened.
#[derive(Default)]
struct Port {
    /// The device it plays, as last applied from the bus.
    device: String,
    /// What that device is called now, if it's plugged in: `device`, or
    /// what the system renumbered it to.
    found: Option<String>,
    connection: Option<Box<dyn Connection>>,
    /// What it has sent, for the Rig panel.
    traffic: Traffic,
    failed_at: Option<Instant>,
    /// The latest moment a message was sent for. What lets go of notes
    /// goes after it.
    latest: Option<Instant>,
    /// What it sends of the patch's clock, as last applied from the bus.
    mode: ClockMode,
    /// Whether its device has been sent Start or Continue, and not Stop since.
    transport_on: bool,
    /// Whether its device has led the clock since it was last sent Start
    /// or Continue (#438). It's brought back into the transport by the
    /// next one, not partway through the song, so a drum machine stopped
    /// by hand stays stopped while Soba plays on.
    led: bool,
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

/// The patch's clock, and each port's share of it still to send.
struct ClockStream {
    /// Its ring and timing. Its `pending` and `held` go unused.
    stream: Stream,
    /// What each port has waiting, in the order it's due.
    queues: [VecDeque<(u64, Message)>; PORTS],
    /// The next tick's place, in ticks from the top, while the clock runs.
    tick: Option<i64>,
}

impl ClockStream {
    fn new(events: Consumer<Outgoing>) -> Self {
        Self { stream: Stream::new(events), queues: Default::default(), tick: None }
    }

    /// Queues `message` for each port that wants it. A port that sends the
    /// transport but missed the Start is brought in on the next sixteenth,
    /// with where the song is and Continue, as a MIDI Out used to when it
    /// started partway in. The ticks before then keep its tempo steady.
    fn fan(&mut self, sample: u64, message: Message, ports: &mut [Port; PORTS]) {
        let bytes = message.bytes();
        match bytes[0] {
            status::START => self.tick = Some(0),
            status::SONG_POSITION => self.tick = Some((bytes[1] as i64 | (bytes[2] as i64) << 7) * 6),
            status::STOP => self.tick = None,
            _ => {}
        }
        for (port, queue) in ports.iter_mut().zip(&mut self.queues) {
            if !port.mode.passes(&message) {
                continue;
            }
            if port.mode == ClockMode::Transport {
                match bytes[0] {
                    status::CLOCK if !port.transport_on && !port.led => {
                        if let Some(tick) = self.tick.filter(|tick| tick % 6 == 0) {
                            if tick == 0 {
                                queue.push_back((sample, Message::real_time(status::START)));
                            } else {
                                queue.push_back((sample, Message::song_position((tick / 6).min(16383) as u16)));
                                queue.push_back((sample, Message::real_time(status::CONTINUE)));
                            }
                            port.transport_on = true;
                        }
                    }
                    status::START | status::CONTINUE => {
                        port.transport_on = true;
                        port.led = false;
                    }
                    status::STOP => port.transport_on = false,
                    _ => {}
                }
            }
            queue.push_back((sample, message));
        }
        if bytes[0] == status::CLOCK {
            if let Some(tick) = &mut self.tick {
                *tick += 1;
            }
        }
    }
}

/// When a message due at `due` goes out on a port offset by `ms`: later by
/// all of it, earlier by no more than `latency`, the output's delay, since
/// a message can't go out before the block it belongs to is made.
fn shifted(due: Instant, ms: f32, latency: Duration) -> Instant {
    let by = Duration::from_secs_f32(ms.abs() / 1000.0);
    if ms >= 0.0 {
        due + by
    } else {
        due.checked_sub(by.min(latency)).unwrap_or(due)
    }
}

impl MidiSender {
    /// A sender for `bus`, playing to `outputs`. The bus hands out rings from now on.
    pub fn new(bus: &'static MidiBus, outputs: Box<dyn Outputs>) -> Self {
        bus.start_draining();
        Self { bus, outputs, streams: Vec::new(), clock: None, ports: Default::default(), devices: Vec::new(), scanned_at: None, ahead: Duration::ZERO, thru: Default::default() }
    }

    /// Sends each message up to `ahead` before it's due, stamped with when
    /// it is: for outputs that keep to a stamp (Web MIDI).
    pub fn ahead(mut self, ahead: Duration) -> Self {
        self.ahead = ahead;
        self
    }

    /// Sends whatever is due, and says when to look again.
    pub fn tick(&mut self, now: Instant) -> Instant {
        self.streams.extend(self.bus.take_new_streams().into_iter().map(Stream::new));
        if let Some(events) = self.bus.take_clock_stream() {
            // The engine made a new one: the audio device changed
            self.clock = Some(ClockStream::new(events));
        }
        if self.bus.take_rescan() || self.scanned_at.is_none_or(|at| now.duration_since(at) >= SCAN_EVERY) {
            self.scanned_at = Some(now);
            self.devices = self.outputs.scan();
            self.bus.set_devices(self.devices.clone());
        }
        self.follow_choices();
        self.find_devices();
        self.follow_clock_modes(now);
        self.forward(now);

        let latency = self.bus.latency();
        let offsets = self.bus.offsets();
        // The clock first, so a Start goes before the notes on its beat
        let mut busy = self.send_clock(now, latency, &offsets);
        let mut index = 0;
        while index < self.streams.len() {
            let gone = self.drain(index, now);
            let stream = &mut self.streams[index];
            let stale = stream.last_block.is_none_or(|at| now.duration_since(at) >= STALE);
            // Everything due goes out; a module that's gone or stopped
            // sends what it left at once, then lets go of its notes
            let flush = gone || stale;
            while let Some(&(sample, message)) = stream.pending.front() {
                let due = stream.due(sample, latency).map(|due| shifted(due, offsets[stream.port], latency));
                if !flush && due.is_none_or(|due| due > now + self.ahead) {
                    break;
                }
                stream.pending.pop_front();
                let port = &mut self.ports[stream.port];
                // What's left of a module that's gone goes after what's on its way
                let at = if flush { port.after(now) } else { due.unwrap_or(now).max(now) };
                Self::send(port, &mut self.outputs, &self.devices, &mut stream.held, message, now, at);
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
        } else if !self.streams.is_empty() || (self.clock.is_some() && self.ports.iter().any(|p| p.mode != ClockMode::Off)) {
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
            self.release_thru(index, now);
            let port = &mut self.ports[index];
            if std::mem::take(&mut port.transport_on) {
                // The device it leaves stops; the new one comes in on a sixteenth
                let at = port.after(now);
                Self::send(port, &mut self.outputs, &self.devices, &mut [0; 16], Message::real_time(status::STOP), now, at);
            }
            self.ports[index] = Port { device, traffic: port.traffic, mode: port.mode, ..Port::default() };
        }
    }

    /// Takes up each port's clock setting. A port that stops sending the
    /// transport while the clock runs is sent Stop, and what it had waiting
    /// that it no longer wants goes. A port whose device leads the clock
    /// is sent none of it, and no Stop: it's playing.
    fn follow_clock_modes(&mut self, now: Instant) {
        let modes = self.bus.clock_modes();
        let led = self.bus.led_ports();
        let Self { ports, clock, outputs, devices, .. } = self;
        for (index, (port, mut mode)) in ports.iter_mut().zip(modes).enumerate() {
            if led & 1 << index != 0 && mode != ClockMode::Off {
                mode = ClockMode::Off;
                port.led = true;
                port.transport_on = false;
            }
            if port.mode == mode {
                continue;
            }
            if let Some(clock) = clock {
                clock.queues[index].retain(|(_, message)| mode.passes(message));
            }
            if mode != ClockMode::Transport && std::mem::take(&mut port.transport_on) {
                let at = port.after(now);
                Self::send(port, outputs, devices, &mut [0; 16], Message::real_time(status::STOP), now, at);
            }
            port.mode = mode;
        }
    }

    /// Takes in the patch's clock, shared out among the ports that want
    /// it, and sends what's due on each. Returns whether it's running.
    fn send_clock(&mut self, now: Instant, latency: Duration, offsets: &[f32; PORTS]) -> bool {
        let Self { clock: slot, ports, outputs, devices, ahead, .. } = self;
        let Some(clock) = slot.as_mut() else { return false };
        while let Ok(event) = clock.stream.events.pop() {
            match event {
                Outgoing::Block { sample, at, rate, .. } => {
                    clock.stream.learn(sample, at, rate);
                    clock.stream.last_block = Some(now);
                }
                Outgoing::Message { sample, message } => clock.fan(sample, message, ports),
            }
        }
        let gone = clock.stream.events.is_abandoned() && clock.stream.events.is_empty();
        let stale = clock.stream.last_block.is_none_or(|at| now.duration_since(at) >= STALE);
        let flush = gone || stale;
        let mut none = [0u128; 16];
        for (index, port) in ports.iter_mut().enumerate() {
            let queue = &mut clock.queues[index];
            while let Some(&(sample, message)) = queue.front() {
                let due = clock.stream.due(sample, latency).map(|due| shifted(due, offsets[index], latency));
                if !flush && due.is_none_or(|due| due > now + *ahead) {
                    break;
                }
                queue.pop_front();
                let at = if flush { port.after(now) } else { due.unwrap_or(now).max(now) };
                Self::send(port, outputs, devices, &mut none, message, now, at);
            }
            if flush && std::mem::take(&mut port.transport_on) {
                // The audio stopped, or the engine went: the device stops too
                let at = port.after(now);
                Self::send(port, outputs, devices, &mut none, Message::real_time(status::STOP), now, at);
            }
        }
        if gone {
            *slot = None;
        }
        !flush
    }

    /// Finds each port's device among those plugged in, by name or by the
    /// name the system renumbered it to. One found under another name
    /// opens afresh.
    fn find_devices(&mut self) {
        let wanted: Vec<String> = self.ports.iter().map(|port| port.device.clone()).collect();
        let found = device_names::find_all(&wanted, &self.devices);
        for (port, found) in self.ports.iter_mut().zip(found) {
            if port.found.as_deref() != found {
                port.found = found.map(str::to_string);
                port.connection = None;
                port.failed_at = None;
            }
        }
    }

    /// Sends `message` on `port` to play at `at`, opening its device first
    /// if need be, and keeps `held` up to date.
    #[allow(clippy::too_many_arguments)]
    fn send(port: &mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], held: &mut [u128; 16], message: Message, now: Instant, at: Instant) {
        if let Some((channel, note)) = message.starts() {
            held[channel as usize] |= 1 << note;
        } else if let Some((channel, note)) = message.ends() {
            held[channel as usize] &= !(1 << note);
        }
        port.latest = Some(port.latest.map_or(at, |latest| latest.max(at)));
        if let Some(connection) = Self::connection(port, outputs, devices, now) {
            match connection.send_at(message.bytes(), at) {
                Ok(()) if message.is_real_time() && message.bytes()[0] == status::CLOCK => {
                    port.traffic.clocks = port.traffic.clocks.wrapping_add(1);
                }
                Ok(()) if message.is_real_time() => {}
                Ok(()) => {
                    port.traffic.messages = port.traffic.messages.wrapping_add(1);
                    port.traffic.last = Some(message);
                }
                Err(error) => {
                    eprintln!("MIDI Out: sending to {} failed: {error}", port.device);
                    port.connection = None;
                    port.failed_at = Some(now);
                }
            }
        }
    }

    /// Lets go of every note in `held` on `port`, with All Notes Off on
    /// their channels.
    fn release(port: &mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], held: &mut [u128; 16], now: Instant) {
        let at = port.after(now);
        for channel in 0..16u8 {
            let notes = std::mem::take(&mut held[channel as usize]);
            if notes == 0 {
                continue;
            }
            let mut ignored = [0u128; 16];
            for note in (0..128u8).filter(|&n| notes & (1 << n) != 0) {
                Self::send(port, outputs, devices, &mut ignored, Message::note_off(channel, note), now, at);
            }
            Self::send(port, outputs, devices, &mut ignored, Message::control(channel, ALL_NOTES_OFF, 0), now, at);
        }
    }

    /// `port`'s open device, opening it if it's plugged in and hasn't just
    /// refused.
    fn connection<'a>(port: &'a mut Port, outputs: &mut Box<dyn Outputs>, devices: &[String], now: Instant) -> Option<&'a mut Box<dyn Connection>> {
        let name = port.found.as_ref().filter(|name| devices.contains(name));
        if let Some(name) = name.filter(|_| port.connection.is_none() && port.failed_at.is_none_or(|at| now.duration_since(at) >= RETRY_AFTER)) {
            match outputs.open(name) {
                Ok(connection) => {
                    port.connection = Some(connection);
                    port.failed_at = None;
                }
                Err(error) => {
                    eprintln!("MIDI Out: couldn't open {name}: {error}");
                    port.failed_at = Some(now);
                }
            }
        }
        port.connection.as_mut()
    }

    /// Tells each MIDI Out's face, and the Rig panel, how its port is doing.
    fn report(&mut self) {
        let clocked = self.clock.as_ref().is_some_and(|clock| clock.stream.last_block.is_some());
        for (index, port) in self.ports.iter_mut().enumerate() {
            // A port with no MIDI Out still sends the clock
            let used = self.streams.iter().any(|s| s.port == index && s.last_block.is_some()) || (clocked && port.mode != ClockMode::Off) || self.thru.uses(index);
            self.bus.set_traffic(index, port.traffic);
            let status = if !used {
                PortStatus::Idle
            } else if port.device.is_empty() {
                PortStatus::NoDevice
            } else if port.found.is_none() {
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

impl Port {
    /// Now, or after the last message sent ahead on this port if that's later.
    fn after(&self, now: Instant) -> Instant {
        self.latest.map_or(now, |latest| latest.max(now))
    }
}

/// The machine's MIDI outputs, through midir. Not in the browser, where
/// midir's Web MIDI would ask for access before anyone clicked.
#[cfg(not(target_arch = "wasm32"))]
pub struct Midir;

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
impl Connection for midir::MidiOutputConnection {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        midir::MidiOutputConnection::send(self, bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// What a test's outputs were sent: (device, bytes, when).
    pub(super) type Log = Arc<Mutex<Vec<(String, Vec<u8>, Instant)>>>;

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
            self.send_at(bytes, Instant::now())
        }
        fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), String> {
            self.log.lock().unwrap().push((self.name.clone(), bytes.to_vec(), at));
            Ok(())
        }
    }

    pub(super) fn setup(refuse: bool) -> (&'static MidiBus, MidiSender, Log) {
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
    fn ahead_sends_early_stamped_with_when_its_due() {
        let (bus, sender, log) = setup(false);
        let mut sender = sender.ahead(Duration::from_millis(100));
        bus.set_latency(Some(Duration::from_millis(30)));
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        // 50 ms in, so heard at 80 ms; and a second later, not yet
        ring.push(Outgoing::Message { sample: 2_400, message: Message::note_on(0, 60, 100) }).unwrap();
        ring.push(Outgoing::Message { sample: 48_000, message: Message::note_off(0, 60) }).unwrap();
        sender.tick(start);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 1, "only what's due within the lookahead");
        assert_eq!(log[0].2, start + Duration::from_millis(80));
    }

    #[test]
    fn letting_go_lands_after_what_was_sent_ahead() {
        let (bus, sender, log) = setup(false);
        let mut sender = sender.ahead(Duration::from_millis(100));
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 4_800, message: Message::note_on(0, 60, 100) }).unwrap();
        sender.tick(start);
        // The patch is deleted before the note sounds
        drop(ring);
        sender.tick(start + TICK);
        let log = log.lock().unwrap();
        let note_on = start + Duration::from_millis(100);
        assert_eq!(log[0].2, note_on);
        assert_eq!(log[1..].iter().map(|(_, b, at)| (b.clone(), *at)).collect::<Vec<_>>(), [
            (vec![0x80, 60, 0], note_on),
            (vec![0xB0, ALL_NOTES_OFF, 0], note_on),
        ]);
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
    fn eight_ports_send_at_once() {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        let log = Log::default();
        let devices: Vec<String> = (0..PORTS).map(|port| format!("Synth {port}")).collect();
        let mut sender = MidiSender::new(bus, Box::new(Recorder { devices: devices.clone(), log: log.clone(), refuse: false }));
        let start = Instant::now();
        let mut rings = Vec::new();
        for (port, device) in devices.iter().enumerate() {
            bus.choose(port, Some(device));
            let mut ring = bus.open_stream().unwrap();
            ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: port as u8 }).unwrap();
            ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 60 + port as u8, 100) }).unwrap();
            rings.push(ring);
        }
        sender.tick(start);
        let log = log.lock().unwrap();
        for (port, device) in devices.iter().enumerate() {
            assert!(log.iter().any(|(to, bytes, _)| to == device && bytes[1] == 60 + port as u8), "port {port}");
            assert_eq!(bus.status(port), PortStatus::Sending);
            assert_eq!(bus.traffic(port).messages, 1);
        }
    }

    #[test]
    fn a_renumbered_device_plays_its_port_again() {
        let bus: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
        let log = Log::default();
        let mut sender = MidiSender::new(bus, Box::new(Recorder { devices: vec!["3- Digitakt".into()], log: log.clone(), refuse: false }));
        bus.choose(2, Some("2- Digitakt"));
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 2 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::control(9, 74, 30) }).unwrap();
        sender.tick(start);
        assert_eq!(log.lock().unwrap()[0].0, "3- Digitakt");
        assert_eq!(bus.status(2), PortStatus::Sending);
        assert_eq!(bus.found()[2].as_deref(), Some("3- Digitakt"));
        assert_eq!(bus.chosen(2).as_deref(), Some("2- Digitakt"), "the choice is kept as it was made");
    }

    #[test]
    fn traffic_counts_clock_apart() {
        let (bus, mut sender, _log) = setup(false);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::real_time(status::START) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::note_on(1, 62, 80) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::real_time(status::CLOCK) }).unwrap();
        ring.push(Outgoing::Message { sample: 0, message: Message::real_time(status::CLOCK) }).unwrap();
        sender.tick(start);
        let traffic = bus.traffic(0);
        assert_eq!((traffic.messages, traffic.clocks), (1, 2));
        assert_eq!(traffic.last, Some(Message::note_on(1, 62, 80)), "clock doesn't hide the note");
    }

    /// A sender playing "Synth" on port A and "DAW" on port B, each port's
    /// clock as `modes` says, and the engine's clock ring.
    fn clocked(modes: [ClockMode; 2]) -> (&'static MidiBus, MidiSender, Log, rtrb::Producer<Outgoing>) {
        let (bus, sender, log) = setup(false);
        bus.choose(1, Some("DAW"));
        bus.set_clock(0, modes[0]);
        bus.set_clock(1, modes[1]);
        let clock = bus.open_clock_stream().unwrap();
        (bus, sender, log, clock)
    }

    /// What `device` was sent: its bytes.
    pub(super) fn sent_to(log: &Log, device: &str) -> Vec<Vec<u8>> {
        log.lock().unwrap().iter().filter(|(d, _, _)| d == device).map(|(_, bytes, _)| bytes.clone()).collect()
    }

    fn real_time(byte: u8) -> Outgoing {
        Outgoing::Message { sample: 0, message: Message::real_time(byte) }
    }

    #[test]
    fn the_clock_goes_once_to_each_port_that_wants_it() {
        let (bus, mut sender, log, mut clock) = clocked([ClockMode::Transport, ClockMode::ClockOnly]);
        bus.choose(2, Some("Synth"));
        let start = Instant::now();
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        for byte in [status::START, status::CLOCK, status::CLOCK, status::STOP] {
            clock.push(real_time(byte)).unwrap();
        }
        // Two MIDI Outs on port A: their notes, and no clock of their own
        let mut rings: Vec<_> = (0..2).map(|_| bus.open_stream().unwrap()).collect();
        for (k, ring) in rings.iter_mut().enumerate() {
            ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
            ring.push(Outgoing::Message { sample: 0, message: Message::note_on(0, 60 + k as u8, 90) }).unwrap();
        }
        sender.tick(start);
        let synth = sent_to(&log, "Synth");
        assert_eq!(synth.iter().filter(|m| m[..] == [status::CLOCK]).count(), 2, "once, however many MIDI Outs");
        assert_eq!(synth[0], [status::START], "the Start before the notes on its beat");
        assert!(synth.contains(&vec![status::STOP]));
        assert_eq!(sent_to(&log, "DAW"), [[status::CLOCK], [status::CLOCK]], "no MIDI Out, still clock; and only clock");
        assert_eq!(bus.status(1), PortStatus::Sending);
        assert_eq!(bus.status(2), PortStatus::Idle, "port C's clock is off");
    }

    #[test]
    fn an_offset_moves_a_ports_notes_and_clock_and_no_others() {
        let (bus, sender, log, mut clock) = clocked([ClockMode::Transport, ClockMode::Transport]);
        let mut sender = sender.ahead(Duration::from_millis(200));
        bus.set_offset(0, 10.0);
        let start = Instant::now();
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(Outgoing::Message { sample: 0, message: Message::real_time(status::START) }).unwrap();
        // 50 ms in
        clock.push(Outgoing::Message { sample: 2_400, message: Message::real_time(status::CLOCK) }).unwrap();
        let mut rings = Vec::new();
        for port in 0..2u8 {
            let mut ring = bus.open_stream().unwrap();
            ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port }).unwrap();
            ring.push(Outgoing::Message { sample: 2_400, message: Message::note_on(0, 60, 90) }).unwrap();
            rings.push(ring);
        }
        sender.tick(start);
        let log = log.lock().unwrap();
        let when = |device: &str, byte: u8| log.iter().find(|(d, b, _)| d == device && b[0] == byte).map(|(_, _, at)| *at - start).unwrap();
        let ms = Duration::from_millis;
        assert_eq!((when("Synth", 0x90), when("Synth", status::CLOCK)), (ms(60), ms(60)), "10 ms later, together");
        assert_eq!((when("DAW", 0x90), when("DAW", status::CLOCK)), (ms(50), ms(50)), "port B as it was");
    }

    #[test]
    fn an_early_offset_goes_no_earlier_than_the_output_delay() {
        let (bus, sender, log, _clock) = clocked([ClockMode::Off, ClockMode::Off]);
        let mut sender = sender.ahead(Duration::from_millis(200));
        bus.set_latency(Some(Duration::from_millis(5)));
        bus.set_offset(0, -20.0);
        assert_eq!(bus.earliest(), -5.0);
        let mut ring = bus.open_stream().unwrap();
        let start = Instant::now();
        ring.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        ring.push(Outgoing::Message { sample: 2_400, message: Message::note_on(0, 60, 90) }).unwrap();
        sender.tick(start);
        assert_eq!(log.lock().unwrap()[0].2 - start, Duration::from_millis(50), "heard at 55, sent 5 early");
    }

    #[test]
    fn a_port_joining_the_song_comes_in_on_a_sixteenth() {
        let (bus, mut sender, log, mut clock) = clocked([ClockMode::Off, ClockMode::Off]);
        let start = Instant::now();
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::START)).unwrap();
        for _ in 0..3 {
            clock.push(real_time(status::CLOCK)).unwrap();
        }
        sender.tick(start);
        bus.set_clock(0, ClockMode::Transport);
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        // Ticks 3 to 7: the sixth, a sixteenth, is tick 6
        for _ in 3..8 {
            clock.push(real_time(status::CLOCK)).unwrap();
        }
        sender.tick(start + TICK);
        let place = Message::song_position(1).bytes().to_vec();
        let tick = vec![status::CLOCK];
        assert_eq!(sent_to(&log, "Synth"), [tick.clone(), tick.clone(), tick.clone(), place, vec![status::CONTINUE], tick.clone(), tick]);
    }

    #[test]
    fn leaving_the_transport_stops_the_device() {
        let (bus, mut sender, log, mut clock) = clocked([ClockMode::Transport, ClockMode::Off]);
        let start = Instant::now();
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::START)).unwrap();
        clock.push(real_time(status::CLOCK)).unwrap();
        sender.tick(start);
        bus.set_clock(0, ClockMode::ClockOnly);
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::CLOCK)).unwrap();
        sender.tick(start + TICK);
        assert_eq!(sent_to(&log, "Synth"), [[status::START], [status::CLOCK], [status::STOP], [status::CLOCK]]);
        // And a clock that goes quiet (the audio stopped) stops a device that runs
        bus.set_clock(0, ClockMode::Transport);
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::START)).unwrap();
        sender.tick(start + TICK * 2);
        sender.tick(start + TICK * 2 + STALE);
        assert_eq!(sent_to(&log, "Synth").last().unwrap(), &[status::STOP]);
    }

    #[test]
    fn the_device_the_clock_follows_is_sent_none_back() {
        // "Synth" on port A sends clock into input 2, and the Clock follows it
        let (bus, mut sender, log, mut clock) = clocked([ClockMode::Transport, ClockMode::Transport]);
        bus.set_same_device(2, 0b01);
        let start = Instant::now();
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::START)).unwrap();
        clock.push(real_time(status::CLOCK)).unwrap();
        sender.tick(start);
        bus.set_leader(Some(2));
        assert_eq!(bus.led_ports(), 0b01);
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        // It restarts the song: Soba's Start goes to the DAW, not back to it
        clock.push(real_time(status::START)).unwrap();
        for _ in 0..12 {
            clock.push(real_time(status::CLOCK)).unwrap();
        }
        sender.tick(start + TICK);
        assert_eq!(sent_to(&log, "Synth"), [[status::START], [status::CLOCK]], "no clock back, and no Stop: it's playing");
        assert_eq!(sent_to(&log, "DAW").len(), 2 + 1 + 12, "the rest of the room follows");

        // It stops, and Soba leads: clock again, but no Continue until Soba's next Start
        bus.set_leader(None);
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        for _ in 0..12 {
            clock.push(real_time(status::CLOCK)).unwrap();
        }
        sender.tick(start + TICK * 2);
        let after = sent_to(&log, "Synth")[2..].to_vec();
        assert_eq!(after, vec![vec![status::CLOCK]; 12], "its tempo, and nothing to start it");
        clock.push(Outgoing::Block { sample: 0, at: start, rate: RATE, port: 0 }).unwrap();
        clock.push(real_time(status::START)).unwrap();
        sender.tick(start + TICK * 3);
        assert_eq!(sent_to(&log, "Synth").last().unwrap(), &[status::START]);
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
