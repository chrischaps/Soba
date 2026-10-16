//! Ableton Link: one tempo, beat and transport shared over the network.
//!
//! A Clock whose Source is **Link** keeps time on a Link session, with Live,
//! Bitwig, phone apps and other copies of Soba on the same network. Every
//! peer plays at one tempo, with its bars lined up, and with start/stop sync
//! on, Play on any of them starts them all.
//!
//! Soba holds one session ([`AblLink`]) for the whole app, made at start-up
//! by [`start`], and a small thread of its own looks after it. The session
//! joins the network only while a Clock follows it: each block, a Link Clock
//! says so ([`pulse`]), and the thread switches the session on while the
//! pulses come and off a second after they stop. It also counts the peers
//! for the Clock's face ([`peers`]), as asking the session isn't safe on the
//! audio thread.
//!
//! The Clock talks to the session on the audio thread with the SDK's audio
//! calls, which never block or allocate (see `modules::clock::link`). Link
//! times are when a sample reaches the speakers, so the output callback
//! reports the device's delay here ([`set_output_latency`]).
//!
//! The renderer and the tests never call [`start`]. With no session, a Link
//! Clock keeps its own time at its Tempo, as an Internal one does.
//!
//! The Link SDK is GPLv2+, so this is behind the `ableton-link` feature,
//! and builds with it are GPL too (see the README).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Once, OnceLock};
use std::time::Duration;

pub use rusty_link::{AblLink, SessionState};

/// Beats in a Link bar: peers line up their bars, `QUANTUM` beats long.
pub const QUANTUM: f64 = 4.0;

/// The tempo a session starts at, until a Clock or a peer sets it.
const START_TEMPO: f64 = 120.0;

/// How often the keeper looks at the pulses and counts the peers.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// How long after the last Link Clock's last block the session leaves the
/// network: long enough not to drop out while a patch reloads.
const LEAVE_AFTER: Duration = Duration::from_secs(1);

static SESSION: OnceLock<AblLink> = OnceLock::new();

/// Bumped once a block by every Clock following Link.
static PULSE: AtomicU32 = AtomicU32::new(0);

/// Peers in the session, other than this one, as the keeper last counted.
static PEERS: AtomicU64 = AtomicU64::new(0);

/// The output device's delay from callback to speaker, in microseconds.
static OUTPUT_LATENCY: AtomicU64 = AtomicU64::new(0);

/// Makes the session, off the network, and starts the thread that looks
/// after it. Once only; later calls do nothing.
pub fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let link = SESSION.get_or_init(|| AblLink::new(START_TEMPO));
        link.enable_start_stop_sync(true);
        let spawned = std::thread::Builder::new().name("soba-ableton-link".into()).spawn(|| keep(link));
        if let Err(error) = spawned {
            eprintln!("Ableton Link: couldn't start: {error}");
        }
    });
}

/// The session, once [`start`] has made it.
///
/// REAL-TIME SAFE: an atomic load.
#[inline]
pub fn session() -> Option<&'static AblLink> {
    SESSION.get()
}

/// Says a Clock following Link ran a block, which keeps the session on the
/// network.
///
/// REAL-TIME SAFE: an atomic add.
#[inline]
pub fn pulse() {
    PULSE.fetch_add(1, Relaxed);
}

/// Peers in the session, not counting Soba itself.
#[inline]
pub fn peers() -> u64 {
    PEERS.load(Relaxed)
}

/// Notes the output device's delay, from callback to speaker.
///
/// REAL-TIME SAFE: an atomic store.
#[inline]
pub fn set_output_latency(delay: Option<Duration>) {
    if let Some(delay) = delay.filter(|d| *d < Duration::from_secs(1)) {
        OUTPUT_LATENCY.store(delay.as_micros() as u64, Relaxed);
    }
}

/// The output device's delay, in microseconds: 0 until a callback says.
#[inline]
pub fn output_latency_micros() -> i64 {
    OUTPUT_LATENCY.load(Relaxed) as i64
}

/// The keeper: on the network while Clocks follow the session, off once
/// they've stopped, and the peers counted.
fn keep(link: &'static AblLink) {
    let mut seen = PULSE.load(Relaxed);
    let mut quiet = LEAVE_AFTER;
    loop {
        std::thread::sleep(LOOK_EVERY);
        let pulse = PULSE.load(Relaxed);
        quiet = if pulse != seen { Duration::ZERO } else { quiet + LOOK_EVERY };
        seen = pulse;
        let wanted = quiet < LEAVE_AFTER;
        if wanted != link.is_enabled() {
            link.enable(wanted);
        }
        PEERS.store(if wanted { link.num_peers() } else { 0 }, Relaxed);
    }
}
