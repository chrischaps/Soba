//! Breeding DX7 voices (#448): a child of one voice, mutated, or of two,
//! crossed. FM is famously hard to program by hand and easy to wander
//! through by ear, so the garden grows by listening.
//!
//! Both keep the voice about as loud as its parents, as Mutate keeps a
//! patch's loudness (#447): a carrier's output level and the peak of its
//! envelope are what's heard as loud, so they stay, and an algorithm only
//! changes to one whose carriers are the same operators. A carrier's ratio
//! is the pitch that's heard, so it stays too, but for a hair of detune,
//! and so does the transpose: the child plays in the same key. What moves
//! is timbre: the modulators' levels and ratios, every envelope's rates,
//! the feedback, the LFO. A fixed-frequency operator stays fixed.
//!
//! Every child is brought into the data sheet's ranges, whatever its
//! parents held: a voice from a file may not be.

use super::dx7::{global, op, wiring, Voice, ALGORITHMS, OPERATORS};
use crate::modules::noise::Pcg32;

/// How far each kind of byte moves at the most, at an amount of 1.
const ENVELOPE: f32 = 30.0;
const LEVEL: f32 = 28.0;
const SCALING: f32 = 20.0;
const FINE: f32 = 12.0;
const LFO: f32 = 22.0;

/// A child of `parent`, mutated by `amount` (0 to 1): a little moves a few
/// bytes a little, all the way moves most of them far. The same seed gives
/// the same child.
pub fn mutate(parent: &Voice, amount: f32, seed: u64) -> Voice {
    let amount = amount.clamp(0.0, 1.0);
    let mut rng = Pcg32::new(seed);
    let mut child = parent.clamped();
    let carriers = child.carriers();
    // How likely a byte is to move, and how far it may
    let p = 0.2 + 0.6 * amount as f64;
    let reach = 0.25 + 1.5 * amount;
    let nudge = |rng: &mut Pcg32, voice: &mut Voice, at: usize, spread: f32| {
        if rng.chance(p) {
            let step = (rng.bipolar() * spread * reach).round() as i32;
            voice.0[at] = (voice.0[at] as i32 + step).clamp(0, super::dx7::max(at) as i32) as u8;
        }
    };
    for n in 1..=OPERATORS {
        let at = Voice::op_at(n);
        let carrier = carriers[n - 1];
        for r in 0..4 {
            nudge(&mut rng, &mut child, at + op::RATES + r, ENVELOPE);
        }
        // A carrier's peak is how loud it is
        for l in if carrier { 1 } else { 0 }..4 {
            nudge(&mut rng, &mut child, at + op::LEVELS + l, ENVELOPE);
        }
        for field in [op::BREAK_POINT, op::LEFT_DEPTH, op::RIGHT_DEPTH] {
            nudge(&mut rng, &mut child, at + field, SCALING);
        }
        for field in [op::RATE_SCALING, op::VELOCITY, op::AMP_MOD] {
            nudge(&mut rng, &mut child, at + field, 1.5);
        }
        for field in [op::LEFT_CURVE, op::RIGHT_CURVE] {
            if rng.chance(0.1 * amount as f64) {
                child.0[at + field] = pick(&mut rng, 4);
            }
        }
        if !carrier {
            nudge(&mut rng, &mut child, at + op::LEVEL, LEVEL);
        }
        // A modulator's ratio a step or two, and its fine part a little
        if !carrier {
            if rng.chance(0.08 + 0.3 * amount as f64) {
                let step = if rng.chance(0.5) { 1 } else { -1 } * if amount > 0.6 && rng.chance(0.4) { 2 } else { 1 };
                child.0[at + op::COARSE] = (child.0[at + op::COARSE] as i32 + step).clamp(0, 31) as u8;
            }
            nudge(&mut rng, &mut child, at + op::FINE, FINE);
        }
        nudge(&mut rng, &mut child, at + op::DETUNE, if carrier { 1.5 } else { 3.0 });
    }

    for r in 0..4 {
        nudge(&mut rng, &mut child, global::PITCH_RATES + r, ENVELOPE / 2.0);
    }
    // The pitch envelope's levels move the note itself: a hair
    for l in 0..4 {
        nudge(&mut rng, &mut child, global::PITCH_LEVELS + l, 2.0);
    }
    if rng.chance(0.25 * amount as f64) {
        let same: Vec<usize> = (0..ALGORITHMS).filter(|&a| a != child.algorithm() && wiring(a).carriers == carriers).collect();
        if !same.is_empty() {
            child.0[global::ALGORITHM] = same[pick(&mut rng, same.len() as u8) as usize] as u8;
        }
    }
    nudge(&mut rng, &mut child, global::FEEDBACK, 2.0);
    for field in [global::LFO_SPEED, global::LFO_DELAY, global::LFO_AMP_DEPTH] {
        nudge(&mut rng, &mut child, field, LFO);
    }
    // Vibrato goes seasick fast
    nudge(&mut rng, &mut child, global::LFO_PITCH_DEPTH, 4.0);
    nudge(&mut rng, &mut child, global::PITCH_MOD, 1.5);
    if rng.chance(0.1 * amount as f64) {
        child.0[global::LFO_WAVE] = pick(&mut rng, 6);
    }
    for field in [global::OSC_SYNC, global::LFO_SYNC] {
        if rng.chance(0.05 * amount as f64) {
            child.0[field] ^= 1;
        }
    }
    // Something always moves: a modulator's level, the brightest thing
    if child == parent.clamped() {
        if let Some(n) = (1..=OPERATORS).find(|&n| !carriers[n - 1]) {
            let at = Voice::op_at(n) + op::LEVEL;
            child.0[at] = if child.0[at] >= 95 { child.0[at] - 4 } else { child.0[at] + 4 };
        } else {
            child.0[global::FEEDBACK] = (child.0[global::FEEDBACK] + 1) % 8;
        }
    }
    child.clamped()
}

/// A child of `a` and `b`: each operator whole from one or the other, the
/// algorithm and feedback, the pitch envelope and the LFO each from one.
/// The carriers' loudness and pitch come with the algorithm, from the
/// parent whose algorithm it is, so neither parent's quiet carrier turns up
/// loud, nor a modulator at fourteen times the note turns up as one heard.
pub fn cross(a: &Voice, b: &Voice, seed: u64) -> Voice {
    let mut rng = Pcg32::new(seed);
    let (a, b) = (a.clamped(), b.clamped());
    let mut child = a;
    let either = |rng: &mut Pcg32| if rng.chance(0.5) { &a } else { &b };
    for n in 1..=OPERATORS {
        let at = Voice::op_at(n);
        let from = either(&mut rng);
        child.0[at..at + 21].copy_from_slice(&from.0[at..at + 21]);
    }
    let shape = either(&mut rng);
    for at in [global::ALGORITHM, global::FEEDBACK, global::OSC_SYNC, global::TRANSPOSE] {
        child.0[at] = shape.0[at];
    }
    let pitch = either(&mut rng);
    child.0[global::PITCH_RATES..global::PITCH_RATES + 8].copy_from_slice(&pitch.0[global::PITCH_RATES..global::PITCH_RATES + 8]);
    let lfo = either(&mut rng);
    child.0[global::LFO_SPEED..global::TRANSPOSE].copy_from_slice(&lfo.0[global::LFO_SPEED..global::TRANSPOSE]);
    for (n, &carrier) in (1..=OPERATORS).zip(child.carriers().iter()) {
        if carrier {
            for field in [op::LEVEL, op::LEVELS, op::MODE, op::COARSE, op::FINE, op::DETUNE] {
                child.set_op(n, field, shape.op(n, field));
            }
        }
    }
    child.clamped()
}

/// A number from 0 to `below` - 1.
fn pick(rng: &mut Pcg32, below: u8) -> u8 {
    (((rng.bipolar() + 1.0) * 0.5 * below as f32) as u8).min(below - 1)
}

/// A mutant's name: its parent's, with its generation at the end
/// ("GLASS BE 3"), in the ten letters a DX7 has.
pub fn mutant_name(parent: &str, generation: u32) -> String {
    let base = match stem(parent) {
        "" => "VOICE",
        stem => stem,
    };
    let mark = generation.to_string();
    let room = 10usize.saturating_sub(mark.len() + 1);
    let base: String = base.chars().take(room).collect();
    format!("{} {mark}", base.trim_end())
}

/// A cross's name: the first half of one parent's and the last of the
/// other's ("SOFT BELL").
pub fn cross_name(a: &str, b: &str) -> String {
    let (a, b) = (stem(a), stem(b));
    let head: String = a.chars().take(5).collect();
    let tail: Vec<char> = b.chars().collect();
    let tail: String = tail[tail.len().saturating_sub(5)..].iter().collect();
    let name = format!("{}{}", head.trim_end(), if tail.starts_with(' ') { tail } else { format!(" {}", tail.trim_start()) });
    name.chars().take(10).collect::<String>().trim_end().to_string()
}

/// A name without the generation a mutant's ends with.
fn stem(name: &str) -> &str {
    let name = name.trim_end();
    match name.rsplit_once(' ') {
        Some((stem, mark)) if !stem.trim().is_empty() && !mark.is_empty() && mark.chars().all(|c| c.is_ascii_digit()) => stem.trim_end(),
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::super::dx7::{self, VOICE_BYTES};
    use super::*;

    /// A voice from `seed`: in range, or any seven bits at all, as a file
    /// might hold.
    fn any_voice(rng: &mut Pcg32, in_range: bool) -> Voice {
        let mut bytes = [0u8; VOICE_BYTES];
        for (at, b) in bytes.iter_mut().enumerate() {
            let top = if in_range { dx7::max(at) } else { 127 };
            *b = pick(rng, top.saturating_add(1).min(128));
            if in_range {
                *b = (*b).max(dx7::min(at));
            }
        }
        Voice(bytes)
    }

    #[test]
    fn mutants_and_crosses_stay_in_range() {
        let mut rng = Pcg32::new(448);
        for k in 0..3000u64 {
            let wild = k % 3 == 0;
            let (a, b) = (any_voice(&mut rng, !wild), any_voice(&mut rng, !wild));
            let amount = (k % 11) as f32 / 10.0;
            let child = mutate(&a, amount, k);
            assert!(child.in_range(), "mutant {k} of {a:?} at {amount}");
            let child = cross(&a, &b, k);
            assert!(child.in_range(), "cross {k}");
            let grandchild = mutate(&cross(&child, &a, k), 1.0, k + 1);
            assert!(grandchild.in_range(), "grandchild {k}");
        }
    }

    #[test]
    fn a_mutant_is_heard_as_loud_and_in_tune_as_its_parent() {
        let mut rng = Pcg32::new(7);
        for k in 0..500u64 {
            let parent = any_voice(&mut rng, true);
            let child = mutate(&parent, 1.0, k);
            assert_ne!(child, parent, "something always moves");
            assert_eq!(child.carriers(), parent.carriers(), "an algorithm with the same carriers");
            for n in (1..=OPERATORS).filter(|&n| parent.carriers()[n - 1]) {
                assert_eq!(child.op(n, op::LEVEL), parent.op(n, op::LEVEL), "carrier {n}'s level");
                assert_eq!(child.op(n, op::LEVELS), parent.op(n, op::LEVELS), "carrier {n}'s peak");
                assert_eq!(child.frequency(n), parent.frequency(n), "carrier {n}'s pitch");
            }
            assert_eq!(child.op(1, op::MODE), parent.op(1, op::MODE));
            assert_eq!(child.0[global::TRANSPOSE], parent.0[global::TRANSPOSE]);
            assert_eq!(child.name(), parent.name(), "the garden names it");
        }
    }

    #[test]
    fn a_little_moves_less_than_a_lot() {
        let parent = Voice::init();
        let moved = |amount: f32| -> u32 { (0..200u64).map(|k| mutate(&parent, amount, k).0.iter().zip(parent.0.iter()).map(|(a, b)| a.abs_diff(*b) as u32).sum::<u32>()).sum() };
        assert!(moved(0.1) * 3 < moved(0.9), "{} against {}", moved(0.1), moved(0.9));
    }

    #[test]
    fn the_same_seed_breeds_the_same_child() {
        let parent = Voice::init();
        assert_eq!(mutate(&parent, 0.5, 9), mutate(&parent, 0.5, 9));
        assert_ne!(mutate(&parent, 0.5, 9), mutate(&parent, 0.5, 10));
    }

    #[test]
    fn a_cross_takes_whole_operators_and_its_algorithms_loudness() {
        let mut rng = Pcg32::new(11);
        for k in 0..300u64 {
            let (a, b) = (any_voice(&mut rng, true), any_voice(&mut rng, true));
            let child = cross(&a, &b, k);
            let shape = if child.algorithm() == a.algorithm() { &a } else { &b };
            for n in 1..=OPERATORS {
                let at = Voice::op_at(n);
                let from_a = child.0[at + op::RATES..at + op::LEVEL] == a.0[at + op::RATES..at + op::LEVEL] && child.0[at + op::COARSE] == a.op(n, op::COARSE);
                let from_b = child.0[at + op::RATES..at + op::LEVEL] == b.0[at + op::RATES..at + op::LEVEL] && child.0[at + op::COARSE] == b.op(n, op::COARSE);
                assert!(from_a || from_b || child.carriers()[n - 1], "operator {n} is one parent's");
                if child.carriers()[n - 1] && a.algorithm() != b.algorithm() {
                    assert_eq!(child.op(n, op::LEVEL), shape.op(n, op::LEVEL), "carrier {n} as loud as in its algorithm's parent");
                    assert_eq!(child.frequency(n), shape.frequency(n), "and at its pitch");
                }
            }
        }
    }

    #[test]
    fn names_carry_the_family() {
        assert_eq!(mutant_name("GLASS BELL", 3), "GLASS BE 3");
        assert_eq!(mutant_name("GLASS BE 3", 4), "GLASS BE 4", "a mutant's mark is replaced");
        assert_eq!(mutant_name("BASS", 12), "BASS 12");
        assert_eq!(mutant_name("", 2), "VOICE 2", "a voice without a name");
        assert_eq!(cross_name("SOFT KEYS", "GLASS BELL"), "SOFT BELL");
        assert_eq!(cross_name("SOFT KEY 4", "WOOD BASS"), "SOFT BASS");
        assert!(cross_name("ABCDEFGHIJ", "KLMNOPQRST").len() <= 10);
    }
}
