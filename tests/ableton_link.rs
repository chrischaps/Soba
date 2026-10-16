//! Two Link Clocks on one machine, as two copies of Soba would be: they find
//! each other, agree on the tempo, keep their bars in phase, and start and
//! stop together.
//!
//! It needs the network (Link finds peers by multicast), so it's ignored by
//! default, and builds only with the feature:
//!
//! ```text
//! cargo test --release --features ableton-link --test ableton_link -- --ignored --nocapture
//! ```

#![cfg(feature = "ableton-link")]

use std::time::{Duration, Instant};

use soba::ableton_link::{self, AblLink, QUANTUM};
use soba::modules::clock::link::{LinkBlock, LinkFollower};

const RATE: f32 = 48_000.0;
const FRAMES: usize = 480;
/// One block of audio: what each loop below waits, as a callback would.
const BLOCK: Duration = Duration::from_millis(10);

/// The two of them, one block each, as two audio callbacks would run.
struct Pair {
    a: (&'static AblLink, LinkFollower),
    b: (&'static AblLink, LinkFollower),
}

impl Pair {
    fn blocks(&mut self, knobs: (f64, f64), runs: (bool, bool)) -> (LinkBlock, LinkBlock) {
        let a = self.a.1.begin_block(self.a.0, knobs.0, runs.0, FRAMES, RATE, 0);
        let b = self.b.1.begin_block(self.b.0, knobs.1, runs.1, FRAMES, RATE, 0);
        std::thread::sleep(BLOCK);
        (a, b)
    }

    /// Runs blocks until `done`, for up to `seconds`.
    fn until(&mut self, knobs: (f64, f64), runs: (bool, bool), seconds: u64, what: &str, done: impl Fn(&LinkBlock, &LinkBlock) -> bool) -> (LinkBlock, LinkBlock) {
        let give_up = Instant::now() + Duration::from_secs(seconds);
        loop {
            let (a, b) = self.blocks(knobs, runs);
            if done(&a, &b) {
                return (a, b);
            }
            assert!(Instant::now() < give_up, "{what}: A {a:?}, B {b:?}");
        }
    }
}

/// How far apart two blocks' bars are, in beats (0 to half a bar): `b`'s
/// phase against `a`'s, carried to `b`'s moment at `a`'s tempo.
fn phase_gap(a: &LinkBlock, b: &LinkBlock) -> f64 {
    let a_then = a.beat + (b.host - a.host) as f64 / 60e6 * a.tempo;
    let gap = (a_then - b.beat).rem_euclid(QUANTUM);
    gap.min(QUANTUM - gap)
}

#[test]
#[ignore = "needs the network: run with --ignored"]
fn two_link_clocks_play_in_time() {
    // A is the app's own session; B a second copy of Soba
    ableton_link::start();
    let a = ableton_link::session().expect("started");
    let b: &'static AblLink = Box::leak(Box::new(AblLink::new(120.0)));
    b.enable_start_stop_sync(true);
    let mut pair = Pair { a: (a, LinkFollower::new()), b: (b, LinkFollower::new()) };

    // A plays alone at 120 until the keeper has it on the network
    pair.until((120.0, 120.0), (true, true), 5, "A starts", |a, _| a.playing);
    b.enable(true);
    let started = Instant::now();
    pair.until((120.0, 120.0), (true, true), 10, "the two find each other", |_, _| ableton_link::peers() == 1 && b.num_peers() == 1);
    println!("found each other in {:?}", started.elapsed());

    // One tempo, and the bars in phase. B joined with its knob at 120 too
    let (x, y) = pair.until((120.0, 120.0), (true, true), 5, "one tempo, in phase", |a, b| {
        (a.tempo - b.tempo).abs() < 1e-3 && a.playing && b.playing && phase_gap(a, b) < 0.01
    });
    println!("in phase: {:.4} beats apart", phase_gap(&x, &y));

    // A's knob moves the session; B follows
    let (x, y) = pair.until((133.0, 120.0), (true, true), 5, "B takes A's tempo", |a, b| a.tempo == 133.0 && (b.tempo - 133.0).abs() < 1e-3);
    assert!(pair.b.1.tempo_changes > 0, "B counts it as a peer's change");
    println!("both at 133: {:.4} beats apart", phase_gap(&x, &y));

    // Bars stay together for a few seconds of play
    let mut worst: f64 = 0.0;
    for _ in 0..300 {
        let (x, y) = pair.blocks((133.0, 133.0), (true, true));
        worst = worst.max(phase_gap(&x, &y));
    }
    println!("over three seconds, at most {worst:.4} beats apart");
    assert!(worst < 0.01, "{worst} beats apart");

    // B's Stop stops A, with A's Run still on; A counts it
    let plays = pair.a.1.play_changes;
    pair.until((133.0, 133.0), (true, false), 5, "B stops A", |a, _| !a.playing);
    assert!(pair.a.1.play_changes > plays);

    // The app moves A's Run to match, which proposes nothing
    for _ in 0..20 {
        let (x, _) = pair.blocks((133.0, 133.0), (false, false));
        assert!(!x.playing);
    }

    // A's Start starts B, with B's Run still off: both from beat 0 together
    let (x, y) = pair.until((133.0, 133.0), (true, false), 5, "A starts B", |a, b| a.playing && b.playing && a.beat >= 0.0 && b.beat >= 0.0);
    println!("started: A at beat {:.3}, B at {:.3}, {:.4} apart", x.beat, y.beat, phase_gap(&x, &y));
    assert!(phase_gap(&x, &y) < 0.01);
    let beat_gap = x.beat + (y.host - x.host) as f64 / 60e6 * x.tempo - y.beat;
    assert!(beat_gap.abs() < 0.05, "they count the same bars: {beat_gap}");
}
