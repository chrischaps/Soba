//! Thru routes (#446), end to end in one process: a MIDI In's callback
//! passes a message down a route, and the sender's thread, woken, sends it
//! to the port's device. The device here is a recorder that notes when each
//! message reached it, so the test measures what forwarding adds: the
//! ring, the wake-up and the send. Well under a millisecond, and the
//! callback's side never allocates.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use soba::link::midi::{MidiBus, PORTS};
use soba::link::midi_sender::{Connection, MidiSender, Outputs};
use soba::link::midi_thru::{ThruBus, ThruRoute};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// When each message reached the device: its bytes and the moment.
type Arrivals = Arc<Mutex<Vec<(Vec<u8>, Instant)>>>;

struct Loopback(Arrivals);
struct Arriving(Arrivals);

impl Outputs for Loopback {
    fn scan(&mut self) -> Vec<String> {
        vec!["MicroFreak".into()]
    }
    fn open(&mut self, _name: &str) -> Result<Box<dyn Connection>, String> {
        Ok(Box::new(Arriving(self.0.clone())))
    }
}

impl Connection for Arriving {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let at = Instant::now();
        self.0.lock().unwrap().push((bytes.to_vec(), at));
        Ok(())
    }
}

/// The routes resolved as the UI does: "Fantom-06" open in slot 3, port B
/// playing the MicroFreak.
fn follow(midi: &MidiBus, thru: &ThruBus) {
    let chosen = midi.all_chosen();
    let found: [Option<String>; PORTS] = std::array::from_fn(|port| Some(chosen[port].clone()).filter(|n| !n.is_empty()));
    thru.follow(|name| (name == "Fantom-06").then_some(3), &chosen, &found);
}

#[test]
fn forwarding_adds_well_under_a_millisecond() {
    let midi: &'static MidiBus = Box::leak(Box::new(MidiBus::new()));
    let thru: &'static ThruBus = Box::leak(Box::new(ThruBus::new()));
    midi.choose(1, Some("MicroFreak"));
    let arrivals = Arrivals::default();
    let mut sender = MidiSender::new(midi, Box::new(Loopback(arrivals.clone()))).with_thru(thru);
    let mut input = thru.open_input().expect("a sender takes the ring");
    thru.edit(|routes| routes.push(ThruRoute { remap: Some(0), ..ThruRoute::new("Fantom-06", 1) }));
    follow(midi, thru);

    // The sender's thread, as the link's: it parks until the next thing is
    // due, and a route passing something wakes it
    let running = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = {
        let running = running.clone();
        std::thread::spawn(move || {
            soba::link::midi_sender::raise_priority();
            thru.set_waker(std::thread::current());
            // The device opens on the first tick, before any timing
            sender.tick(Instant::now());
            ready_tx.send(()).unwrap();
            while running.load(Ordering::Relaxed) {
                let now = Instant::now();
                sender.forward(now);
                let next = sender.tick(now);
                std::thread::park_timeout(next.saturating_duration_since(Instant::now()));
            }
        })
    };
    ready_rx.recv().unwrap();

    const NOTES: usize = 200;
    let mut delays = Vec::with_capacity(NOTES);
    let mut allocations = 0;
    for k in 0..NOTES {
        // Played a few milliseconds apart, so the thread is asleep, as it
        // is between a player's notes
        std::thread::sleep(Duration::from_millis(2 + (k % 3) as u64));
        let note = 36 + (k % 48) as u8;
        let played = Instant::now();
        allocations += count(|| assert!(input.forward(3, &[0x95, note, 100])));
        let arrived = loop {
            if let Some(&(_, at)) = arrivals.lock().unwrap().get(k) {
                break at;
            }
            assert!(played.elapsed() < Duration::from_millis(500), "note {k} never arrived");
            std::hint::spin_loop();
        };
        delays.push(arrived.duration_since(played));
    }
    running.store(false, Ordering::Relaxed);
    thread.thread().unpark();
    thread.join().unwrap();

    assert_eq!(allocations, 0, "the input's callback never allocates");
    let arrivals = arrivals.lock().unwrap();
    assert!(arrivals.iter().enumerate().all(|(k, (bytes, _))| bytes[..] == [0x90, 36 + (k % 48) as u8, 100]), "each in order, moved to channel 1");

    delays.sort();
    let median = delays[NOTES / 2];
    let p95 = delays[NOTES * 95 / 100];
    println!("thru loopback: median {median:?}, 95th percentile {p95:?}, worst {:?}", delays[NOTES - 1]);
    assert!(median < Duration::from_micros(500), "median {median:?}");
    assert!(p95 < Duration::from_millis(1), "95th percentile {p95:?}");
}

/// Allocations and frees made by `f` on this thread.
fn count(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCATIONS.load(Ordering::Relaxed) - before
}
