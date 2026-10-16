//! The sender thread: reads the [`LinkBus`] every 5 ms and sends each port
//! that has a Link Out on it one bundle of every lane.
//!
//! Each bundle carries every lane, so a dropped packet is repaired by the
//! next one 5 ms later. A lane nothing has written for a while (its cable
//! pulled, its module deleted) is sent as 0, and a port whose Link Outs
//! have all gone quiet is sent nothing, so Seoul sees the link drop.
//!
//! Gates are sent as edges, not samples. A 1 ms trigger can rise and fall
//! between two sends, so a rising edge the audio thread saw is always sent
//! as a 1 for one bundle, and a gate that fell and rose again between sends
//! is sent as a 0 and then a 1, so Seoul counts both.
//!
//! The socket is connected to its port, so when nothing listens there the
//! system says so (the ICMP "port unreachable" comes back as a refused
//! receive), and a Link Out's face can tell you Seoul isn't running.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Once;
use std::time::{Duration, Instant};

use super::osc::{Arg, BundleWriter};
use super::{LinkBus, LinkStatus, SlotReading, BUS, CV_LANES, GATE_LANES, PORTS, VERSION};

/// How often a bundle goes out: about 200 Hz.
pub const TICK: Duration = Duration::from_millis(5);
/// How often the bus is looked at while no Link Out runs.
const IDLE_TICK: Duration = Duration::from_millis(50);
/// A lane, or a port, not written for this long has been left behind. A
/// block is 10 ms or so, but a busy machine or a device with a long period
/// can stall for many, and a held gate taken for dropped would rise again
/// when it came back: a false hit. Well inside Seoul's 500 ms, so a
/// deleted Link Out still drops the link.
const STALE: Duration = Duration::from_millis(250);
/// How often `/soba/hello` goes out.
const HELLO_EVERY: Duration = Duration::from_secs(1);
/// How long a refused send says no one is listening.
const REFUSED_FOR: Duration = Duration::from_millis(1500);

/// Starts the sender thread for [`BUS`], once however often it's called.
pub fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new().name("soba-link".into()).spawn(|| Sender::new(&BUS, PORTS).run());
        if let Err(error) = spawned {
            eprintln!("Link Out: couldn't start the sender: {error}");
        }
    });
}

/// Sends a bus's ports.
pub struct Sender {
    bus: &'static LinkBus,
    ports: [u16; PORTS.len()],
    states: [PortState; PORTS.len()],
    writer: BundleWriter,
}

impl Sender {
    /// A sender for `bus`, its slots going to `ports` on this machine.
    pub fn new(bus: &'static LinkBus, ports: [u16; PORTS.len()]) -> Self {
        Self { bus, ports, states: Default::default(), writer: BundleWriter::new() }
    }

    /// Sends forever, every [`TICK`] while any port is in use.
    pub fn run(mut self) -> ! {
        let mut next = Instant::now();
        loop {
            let now = Instant::now();
            let busy = self.tick(now);
            // Ticks keep to a schedule, so sending doesn't stretch them; one
            // that has fallen behind (the machine was asleep) starts afresh
            next += if busy { TICK } else { IDLE_TICK };
            if next < now {
                next = now + TICK;
            }
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }

    /// Sends each port in use its bundle. Returns whether any was.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut busy = false;
        for ((slot, state), &port) in self.bus.slots().iter().zip(&mut self.states).zip(&self.ports) {
            if !state.compose(&slot.read(), now, &mut self.writer) {
                if state.socket.take().is_some() {
                    slot.set_status(LinkStatus::Idle);
                }
                continue;
            }
            busy = true;
            slot.set_status(state.send(port, self.writer.bytes(), now));
        }
        busy
    }
}

/// What sending one port remembers between ticks.
#[derive(Default)]
struct PortState {
    /// Each counter as last read, and when it last moved.
    pulse: Seen,
    cv: [Seen; CV_LANES],
    gate: [Seen; GATE_LANES],
    /// Rising edges counted so far, as last read.
    gate_edges: [u32; GATE_LANES],
    /// Each gate as last sent, and whether it must be sent high next
    /// (it fell and rose between two sends, and was sent low for it).
    gate_sent: [bool; GATE_LANES],
    gate_rises_next: [bool; GATE_LANES],
    hello_at: Option<Instant>,
    socket: Option<UdpSocket>,
    refused_at: Option<Instant>,
}

/// A counter as last read, and when it last moved.
#[derive(Clone, Copy, Default)]
struct Seen {
    count: u32,
    moved_at: Option<Instant>,
}

impl Seen {
    /// Reads the counter's new value: whether it has moved within [`STALE`].
    fn fresh(&mut self, count: u32, now: Instant) -> bool {
        if count != self.count {
            self.count = count;
            self.moved_at = Some(now);
        }
        self.moved_at.is_some_and(|at| now.duration_since(at) < STALE)
    }
}

impl PortState {
    /// Writes the bundle for `reading` into `writer`. Returns false, and
    /// writes nothing, when no Link Out has run on the port lately.
    fn compose(&mut self, reading: &SlotReading, now: Instant, writer: &mut BundleWriter) -> bool {
        if !self.pulse.fresh(reading.pulse, now) {
            // Gone: start afresh if one comes back
            *self = PortState { socket: self.socket.take(), ..PortState::default() };
            return false;
        }
        writer.begin();
        for lane in 0..CV_LANES {
            let value = if self.cv[lane].fresh(reading.cv_written[lane], now) { reading.cv[lane] } else { 0.0 };
            writer.message(CV_ADDRESSES[lane], &[Arg::Float(if value.is_finite() { value } else { 0.0 })]);
        }
        for lane in 0..GATE_LANES {
            let live = self.gate[lane].fresh(reading.gate_written[lane], now);
            let edges = reading.gate_edges[lane];
            let rose = edges != self.gate_edges[lane];
            self.gate_edges[lane] = edges;
            let high = if std::mem::take(&mut self.gate_rises_next[lane]) {
                true
            } else if rose && self.gate_sent[lane] {
                // It fell and rose again since the last send: a low now, so
                // Seoul sees this rise as one
                self.gate_rises_next[lane] = true;
                false
            } else {
                // A rise is sent even when the gate has fallen again since
                rose || (live && reading.gate[lane])
            };
            self.gate_sent[lane] = high;
            writer.message(GATE_ADDRESSES[lane], &[Arg::Int(high as i32)]);
        }
        // With no Clock the patch plays all the time it's running
        let playing = reading.clock.is_none_or(|clock| clock.playing);
        if let Some(clock) = reading.clock {
            let (bpm, phase, beat, bar) = clock.message();
            writer.message("/soba/clock", &[Arg::Float(bpm), Arg::Float(phase), Arg::Int(beat), Arg::Int(bar)]);
        }
        writer.message("/soba/transport", &[Arg::Int(playing as i32)]);
        if self.hello_at.is_none_or(|at| now.duration_since(at) >= HELLO_EVERY) {
            self.hello_at = Some(now);
            writer.message("/soba/hello", &[Arg::Str(VERSION.into())]);
        }
        true
    }

    /// Sends `bundle` to `port`, opening the socket first if need be, and
    /// says how it went.
    fn send(&mut self, port: u16, bundle: &[u8], now: Instant) -> LinkStatus {
        if self.socket.is_none() {
            match open(port) {
                Ok(socket) => self.socket = Some(socket),
                Err(error) => {
                    eprintln!("Link Out: couldn't open a socket to port {port}: {error}");
                    return LinkStatus::Failed;
                }
            }
        }
        let Some(socket) = &self.socket else { return LinkStatus::Failed };
        // A refusal from the last send arrives as an error on the next call
        let mut scratch = [0u8; 64];
        let heard = socket.recv(&mut scratch);
        let sent = socket.send(bundle);
        for result in [heard.map(|_| ()), sent.map(|_| ())] {
            match result {
                Err(error) if refused(error.kind()) => self.refused_at = Some(now),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    eprintln!("Link Out: sending to port {port} failed: {error}");
                    self.socket = None;
                    return LinkStatus::Failed;
                }
                Ok(()) => {}
            }
        }
        if self.refused_at.is_some_and(|at| now.duration_since(at) < REFUSED_FOR) {
            LinkStatus::NoListener
        } else {
            LinkStatus::Sending
        }
    }
}

/// Whether an error is the port saying no one listens there.
fn refused(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset)
}

/// A socket on the loopback interface, connected to `port` there. Bound to
/// 127.0.0.1 only, so the firewall never asks.
fn open(port: u16) -> std::io::Result<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    socket.connect((Ipv4Addr::LOCALHOST, port))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

const CV_ADDRESSES: [&str; CV_LANES] =
    ["/soba/cv/1", "/soba/cv/2", "/soba/cv/3", "/soba/cv/4", "/soba/cv/5", "/soba/cv/6", "/soba/cv/7", "/soba/cv/8"];
const GATE_ADDRESSES: [&str; GATE_LANES] = ["/soba/gate/1", "/soba/gate/2", "/soba/gate/3", "/soba/gate/4"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::osc::decode;
    use crate::link::ClockState;

    /// A bus of its own, so tests don't meet each other or a running Soba.
    fn bus() -> &'static LinkBus {
        Box::leak(Box::new(LinkBus::new()))
    }

    fn compose(state: &mut PortState, bus: &LinkBus, now: Instant) -> Option<Vec<(String, Vec<Arg>)>> {
        let mut writer = BundleWriter::new();
        state.compose(&bus.slot(0).read(), now, &mut writer).then(|| decode(writer.bytes()).unwrap())
    }

    fn arg(messages: &[(String, Vec<Arg>)], address: &str) -> Option<Arg> {
        messages.iter().find(|(a, _)| a == address).map(|(_, args)| args[0].clone())
    }

    #[test]
    fn nothing_is_sent_until_a_link_out_runs() {
        let bus = bus();
        let mut state = PortState::default();
        let now = Instant::now();
        assert!(compose(&mut state, bus, now).is_none());
        bus.slot(0).pulse();
        let messages = compose(&mut state, bus, now).unwrap();
        // Every lane, the transport, and hello the first time
        assert_eq!(messages.len(), CV_LANES + GATE_LANES + 2);
        assert_eq!(arg(&messages, "/soba/hello"), Some(Arg::Str(VERSION.into())));
        assert_eq!(arg(&messages, "/soba/transport"), Some(Arg::Int(1)), "no Clock plays all the time");
        // Hello again only once a second has passed
        bus.slot(0).pulse();
        let later = compose(&mut state, bus, now + TICK).unwrap();
        assert!(arg(&later, "/soba/hello").is_none());
        bus.slot(0).pulse();
        assert!(arg(&compose(&mut state, bus, now + HELLO_EVERY).unwrap(), "/soba/hello").is_some());
    }

    #[test]
    fn a_port_left_behind_goes_quiet() {
        let bus = bus();
        let mut state = PortState::default();
        let now = Instant::now();
        bus.slot(0).pulse();
        assert!(compose(&mut state, bus, now).is_some());
        assert!(compose(&mut state, bus, now + STALE / 2).is_some(), "a slow block is no reason to stop");
        assert!(compose(&mut state, bus, now + STALE).is_none());
    }

    #[test]
    fn a_lane_left_behind_reads_zero() {
        let bus = bus();
        let slot = bus.slot(0);
        let mut state = PortState::default();
        let now = Instant::now();
        slot.pulse();
        slot.set_cv(0, 0.75);
        slot.set_cv(7, -0.25);
        let messages = compose(&mut state, bus, now).unwrap();
        assert_eq!(arg(&messages, "/soba/cv/1"), Some(Arg::Float(0.75)));
        assert_eq!(arg(&messages, "/soba/cv/8"), Some(Arg::Float(-0.25)));
        assert_eq!(arg(&messages, "/soba/cv/2"), Some(Arg::Float(0.0)));
        // Lane 8's cable is pulled: only lane 1 is still written
        let later = now + STALE + TICK;
        slot.pulse();
        slot.set_cv(0, 0.5);
        let messages = compose(&mut state, bus, later).unwrap();
        assert_eq!(arg(&messages, "/soba/cv/1"), Some(Arg::Float(0.5)));
        assert_eq!(arg(&messages, "/soba/cv/8"), Some(Arg::Float(0.0)));
    }

    #[test]
    fn a_trigger_between_sends_is_sent() {
        let bus = bus();
        let slot = bus.slot(0);
        let mut state = PortState::default();
        let mut now = Instant::now();
        let gate = |state: &mut PortState, now: Instant| {
            slot.pulse();
            match arg(&compose(state, bus, now).unwrap(), "/soba/gate/1") {
                Some(Arg::Int(value)) => value,
                other => panic!("{other:?}"),
            }
        };
        slot.set_gate(0, false, 0);
        assert_eq!(gate(&mut state, now), 0);
        // A 1 ms trigger rose and fell inside one block
        slot.set_gate(0, false, 1);
        now += TICK;
        assert_eq!(gate(&mut state, now), 1, "the rise is sent");
        now += TICK;
        assert_eq!(gate(&mut state, now), 0);
        // A held gate is sent high while it's held
        slot.set_gate(0, true, 1);
        now += TICK;
        assert_eq!(gate(&mut state, now), 1);
        now += TICK;
        assert_eq!(gate(&mut state, now), 1);
        // It fell and rose again between sends: low, then high
        slot.set_gate(0, true, 1);
        now += TICK;
        assert_eq!(gate(&mut state, now), 0);
        now += TICK;
        assert_eq!(gate(&mut state, now), 1);
        now += TICK;
        assert_eq!(gate(&mut state, now), 1);
    }

    #[test]
    fn the_clock_is_sent_when_the_patch_has_one() {
        let bus = bus();
        let slot = bus.slot(0);
        let mut state = PortState::default();
        slot.pulse();
        slot.set_clock(Some(ClockState { playing: false, bpm: 96.0, beat: 13.5, beats_per_bar: 4 }));
        let messages = compose(&mut state, bus, Instant::now()).unwrap();
        let clock = messages.iter().find(|(a, _)| a == "/soba/clock").unwrap();
        assert_eq!(clock.1, vec![Arg::Float(96.0), Arg::Float(0.5), Arg::Int(13), Arg::Int(3)]);
        assert_eq!(arg(&messages, "/soba/transport"), Some(Arg::Int(0)), "the Clock is stopped");
    }

    #[test]
    fn bundles_reach_a_listener() {
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = listener.local_addr().unwrap().port();
        let bus = bus();
        let mut sender = Sender::new(bus, [port, 1, 2, 3]);
        let now = Instant::now();
        assert!(!sender.tick(now), "nothing to send yet");
        bus.slot(0).pulse();
        bus.slot(0).set_cv(2, 0.125);
        assert!(sender.tick(now));
        let mut buffer = [0u8; 2048];
        let length = listener.recv(&mut buffer).unwrap();
        let messages = decode(&buffer[..length]).unwrap();
        assert_eq!(arg(&messages, "/soba/cv/3"), Some(Arg::Float(0.125)));
        assert_eq!(bus.slot(0).status(), LinkStatus::Sending);
        assert_eq!(bus.slot(1).status(), LinkStatus::Idle, "ports with no Link Out send nothing");
    }

    #[test]
    fn a_closed_port_says_no_one_listens() {
        // A port that was free a moment ago, and nothing listening on it
        let port = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        let bus = bus();
        let mut sender = Sender::new(bus, [port, 1, 2, 3]);
        let mut now = Instant::now();
        let mut status = LinkStatus::Idle;
        for _ in 0..40 {
            bus.slot(0).pulse();
            sender.tick(now);
            status = bus.slot(0).status();
            if status == LinkStatus::NoListener {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            now += TICK;
        }
        assert_eq!(status, LinkStatus::NoListener);
    }
}
