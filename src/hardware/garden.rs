//! An FM Voice's garden (#448): the voices bred so far, each with the one
//! or two it came from, the one playing, and the one picked to cross it
//! with. It holds a DX7 bank's worth, 32, so the whole garden saves as a
//! bank and loads into a volca's memories in its order.
//!
//! The node keeps it in its labels, as a Setlist keeps its rows, so it
//! saves, copies and undoes with the rest of the node: `"garden"` holds it
//! all as JSON, each voice as hex. A node with no garden yet has the seeds
//! ([`seeds`]), Soft Keys playing.
//!
//! When it's full, a new voice takes the place of the oldest one that
//! isn't playing, picked, or kept: keep a voice to see it through every
//! weeding.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::breeding;
use super::dx7::{op, Voice, BANK_VOICES};

/// The label a garden is kept in.
pub const KEY: &str = "garden";

/// The most voices a garden holds: a bank.
pub const ROOM: usize = BANK_VOICES;

/// How a voice came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// One of the seeds a new node starts with.
    Seed,
    /// From a `.syx` file.
    Loaded,
    /// Mutated from one parent.
    Mutant,
    /// Crossed from two.
    Cross,
}

/// One voice in the garden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grown {
    /// Its own number, never given twice in a garden.
    pub id: u32,
    pub voice: Voice,
    /// The ids of the voices it came from: none for a seed or a file's.
    /// Gone from the garden, a parent is remembered only by its number.
    pub parents: Vec<u32>,
    pub origin: Origin,
    /// Its generation: 0 for a seed or a file's, one more than its eldest
    /// parent's for a child.
    pub generation: u32,
    /// Kept through every weeding.
    pub kept: bool,
}

/// What the node keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Garden {
    /// Oldest first.
    pub voices: Vec<Grown>,
    /// The voice playing.
    pub current: u32,
    /// The voice picked to cross the playing one with.
    pub partner: Option<u32>,
    /// The next id to give.
    pub next: u32,
}

/// How a garden is written down.
#[derive(Serialize, Deserialize)]
struct Written {
    current: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partner: Option<u32>,
    next: u32,
    voices: Vec<WrittenVoice>,
}

#[derive(Serialize, Deserialize)]
struct WrittenVoice {
    id: u32,
    voice: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    from: Vec<u32>,
    how: Origin,
    #[serde(default, skip_serializing_if = "is_zero")]
    generation: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    kept: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Garden {
    /// The garden in a node's labels, or the seeds.
    pub fn of(labels: &BTreeMap<String, String>) -> Self {
        labels.get(KEY).and_then(|text| Self::read(text)).unwrap_or_else(Self::seeded)
    }

    /// A garden of the seeds, Soft Keys playing.
    pub fn seeded() -> Self {
        Self::of_voices(seeds(), Origin::Seed)
    }

    /// A garden of `voices` (the first 32), the first playing.
    pub fn of_voices(voices: Vec<Voice>, origin: Origin) -> Self {
        let voices: Vec<Grown> = voices.into_iter().take(ROOM).enumerate().map(|(k, voice)| Grown { id: k as u32, voice, parents: Vec::new(), origin, generation: 0, kept: false }).collect();
        Self { next: voices.len() as u32, voices, current: 0, partner: None }
    }

    fn read(text: &str) -> Option<Self> {
        let written: Written = serde_json::from_str(text).ok()?;
        let voices: Vec<Grown> = written
            .voices
            .into_iter()
            .filter_map(|w| Some(Grown { id: w.id, voice: Voice::from_hex(&w.voice)?, parents: w.from, origin: w.how, generation: w.generation, kept: w.kept }))
            .take(ROOM)
            .collect();
        let first = voices.first()?.id;
        let has = |id: u32| voices.iter().any(|g| g.id == id);
        let current = if has(written.current) { written.current } else { first };
        let partner = written.partner.filter(|&p| has(p) && p != current);
        let next = written.next.max(voices.iter().map(|g| g.id + 1).max().unwrap_or(0));
        Some(Self { voices, current, partner, next })
    }

    /// The garden as the node keeps it.
    pub fn written(&self) -> String {
        let written = Written {
            current: self.current,
            partner: self.partner,
            next: self.next,
            voices: self
                .voices
                .iter()
                .map(|g| WrittenVoice { id: g.id, voice: g.voice.to_hex(), from: g.parents.clone(), how: g.origin, generation: g.generation, kept: g.kept })
                .collect(),
        };
        serde_json::to_string(&written).unwrap_or_default()
    }

    /// The labels that keep it.
    pub fn labels(&self) -> Vec<(String, String)> {
        vec![(KEY.to_string(), self.written())]
    }

    pub fn get(&self, id: u32) -> Option<&Grown> {
        self.voices.iter().find(|g| g.id == id)
    }

    /// The voice playing.
    pub fn playing(&self) -> &Grown {
        self.get(self.current).unwrap_or(&self.voices[0])
    }

    /// Where `id` is in the garden: its place in a bank, from 0.
    pub fn place(&self, id: u32) -> Option<usize> {
        self.voices.iter().position(|g| g.id == id)
    }

    /// Plays `id`.
    pub fn recall(&mut self, id: u32) {
        if self.get(id).is_some() {
            self.current = id;
            if self.partner == Some(id) {
                self.partner = None;
            }
        }
    }

    /// Picks `id` to cross the playing voice with, or no longer.
    pub fn pick(&mut self, id: u32) {
        self.partner = if self.partner == Some(id) || id == self.current || self.get(id).is_none() { None } else { Some(id) };
    }

    pub fn keep(&mut self, id: u32, kept: bool) {
        if let Some(g) = self.voices.iter_mut().find(|g| g.id == id) {
            g.kept = kept;
        }
    }

    /// Whether a new voice has room: there's space, or one to weed.
    pub fn has_room(&self) -> bool {
        self.voices.len() < ROOM || self.weed().is_some()
    }

    /// The voice the next one would replace, once the garden's full: the
    /// oldest that isn't playing, picked or kept.
    pub fn weed(&self) -> Option<usize> {
        if self.voices.len() < ROOM {
            return None;
        }
        self.voices.iter().position(|g| g.id != self.current && Some(g.id) != self.partner && !g.kept)
    }

    /// Grows `voice` and plays it. Full, it takes the weeded voice's place
    /// (so the bank's order stays as it was); full of kept voices, it isn't
    /// grown, and the answer is None.
    fn grow(&mut self, voice: Voice, parents: Vec<u32>, origin: Origin) -> Option<u32> {
        let generation = match parents.is_empty() {
            true => 0,
            false => parents.iter().filter_map(|&p| self.get(p)).map(|g| g.generation + 1).max().unwrap_or(1),
        };
        let id = self.next;
        let grown = Grown { id, voice, parents, origin, generation, kept: false };
        match self.weed() {
            Some(place) => self.voices[place] = grown,
            None if self.voices.len() < ROOM => self.voices.push(grown),
            None => return None,
        }
        self.next += 1;
        self.current = id;
        Some(id)
    }

    /// A mutant of the playing voice, playing: `amount` from 0 to 1.
    pub fn mutate(&mut self, amount: f32, seed: u64) -> Option<u32> {
        let parent = self.playing().clone();
        let mut child = breeding::mutate(&parent.voice, amount, seed);
        child.set_name(&breeding::mutant_name(&parent.voice.name(), parent.generation + 1));
        self.grow(child, vec![parent.id], Origin::Mutant)
    }

    /// A cross of the playing voice and the picked one, playing. The picked
    /// one stays picked, to cross again.
    pub fn cross(&mut self, seed: u64) -> Option<u32> {
        let a = self.playing().clone();
        let b = self.get(self.partner?)?.clone();
        let mut child = breeding::cross(&a.voice, &b.voice, seed);
        child.set_name(&breeding::cross_name(&a.voice.name(), &b.voice.name()));
        self.grow(child, vec![a.id, b.id], Origin::Cross)
    }

    /// Voices from a file, each as a voice of its own, after those already
    /// growing and in the file's order: the first of them playing. To make
    /// room the oldest voices go, any but a kept or picked one, so a bank
    /// loaded whole is a garden of its own. As many as there's room for;
    /// the answer is how many.
    pub fn plant(&mut self, voices: &[Voice]) -> usize {
        let stays = |g: &Grown, partner: Option<u32>| g.kept || Some(g.id) == partner;
        let room = ROOM - self.voices.iter().filter(|g| stays(g, self.partner)).count();
        let planted = voices.len().min(room);
        if planted == 0 {
            return 0;
        }
        let mut over = (self.voices.len() + planted).saturating_sub(ROOM);
        let partner = self.partner;
        self.voices.retain(|g| {
            let goes = over > 0 && !stays(g, partner);
            over -= goes as usize;
            !goes
        });
        self.current = self.next;
        for voice in &voices[..planted] {
            self.voices.push(Grown { id: self.next, voice: *voice, parents: Vec::new(), origin: Origin::Loaded, generation: 0, kept: false });
            self.next += 1;
        }
        planted
    }

    /// The garden's voices in bank order, for a bank.
    pub fn bank(&self) -> Vec<Voice> {
        self.voices.iter().map(|g| g.voice).collect()
    }

    /// How the playing voice came to be, in a few words: "Mutant of SOFT
    /// KEYS (2)", "Cross of 3 and 7".
    pub fn lineage(&self, id: u32) -> String {
        let Some(g) = self.get(id) else { return String::new() };
        let named = |p: u32| match (self.get(p), self.place(p)) {
            (Some(parent), Some(place)) => format!("{} ({})", parent.voice.name(), place + 1),
            _ => "a voice weeded out".to_string(),
        };
        match g.origin {
            Origin::Seed => "A seed".to_string(),
            Origin::Loaded => "From a file".to_string(),
            Origin::Mutant => format!("Mutant of {}", g.parents.first().map_or_else(String::new, |&p| named(p))),
            Origin::Cross => format!("Cross of {}", g.parents.iter().map(|&p| named(p)).collect::<Vec<_>>().join(" and ")),
        }
    }
}

/// A voice's brightness, 0 to 1: how hard its modulators drive, which is
/// most of what FM's timbre is. For a chip's colour.
pub fn brightness(voice: &Voice) -> f32 {
    let carriers = voice.carriers();
    let levels: Vec<f32> = (1..=6).filter(|&n| !carriers[n - 1]).map(|n| voice.op(n, op::LEVEL) as f32 / 99.0).collect();
    let feedback = voice.feedback() as f32 / 7.0;
    if levels.is_empty() {
        return 0.15 + 0.5 * feedback;
    }
    let drive = levels.iter().map(|l| l * l).sum::<f32>() / levels.len() as f32;
    (drive.sqrt() * 0.85 + feedback * 0.15).clamp(0.0, 1.0)
}

/// The seeds a garden starts with. Soft Keys, Glass Bell and Wood Bass
/// are Soba's own, made for this, and INIT VOICE is the DX7's blank page.
pub fn seeds() -> Vec<Voice> {
    vec![soft_keys(), glass_bell(), wood_bass(), Voice::init()]
}

/// Sets operator `n`: its envelope's rates and levels, its ratio (coarse
/// and fine), its output level and how far velocity moves it.
fn operator(voice: &mut Voice, n: usize, rates: [u8; 4], levels: [u8; 4], (coarse, fine): (u8, u8), level: u8, velocity: u8) {
    let at = Voice::op_at(n);
    voice.0[at + op::RATES..at + op::RATES + 4].copy_from_slice(&rates);
    voice.0[at + op::LEVELS..at + op::LEVELS + 4].copy_from_slice(&levels);
    voice.set_op(n, op::COARSE, coarse);
    voice.set_op(n, op::FINE, fine);
    voice.set_op(n, op::LEVEL, level);
    voice.set_op(n, op::VELOCITY, velocity);
    voice.set_op(n, op::RATE_SCALING, 2);
}

/// Three two-operator pairs (algorithm 5): a round body, a struck tine at
/// fourteen times the note, and a detuned shimmer, brighter played harder.
fn soft_keys() -> Voice {
    use super::dx7::global;
    let mut v = Voice::init();
    v.0[global::ALGORITHM] = 4;
    v.0[global::FEEDBACK] = 5;
    operator(&mut v, 1, [96, 25, 25, 67], [99, 75, 0, 0], (1, 0), 96, 2);
    operator(&mut v, 2, [95, 50, 35, 78], [99, 75, 0, 0], (1, 0), 58, 7);
    operator(&mut v, 3, [95, 20, 20, 50], [99, 92, 0, 0], (1, 0), 88, 2);
    operator(&mut v, 4, [98, 71, 50, 70], [99, 0, 0, 0], (14, 0), 62, 6);
    operator(&mut v, 5, [80, 20, 20, 60], [99, 90, 0, 0], (1, 0), 78, 2);
    operator(&mut v, 6, [95, 30, 20, 60], [99, 70, 0, 0], (1, 0), 66, 4);
    v.set_op(3, op::DETUNE, 10);
    v.set_op(5, op::DETUNE, 4);
    v.set_name("SOFT KEYS");
    v
}

/// A bell (algorithm 5): each carrier struck by a modulator at an
/// inharmonic ratio, 3.5 and 5, ringing long, with a low hum beneath.
fn glass_bell() -> Voice {
    use super::dx7::global;
    let mut v = Voice::init();
    v.0[global::ALGORITHM] = 4;
    operator(&mut v, 1, [99, 30, 22, 35], [99, 70, 0, 0], (1, 0), 95, 2);
    operator(&mut v, 2, [99, 34, 25, 40], [99, 60, 0, 0], (2, 75), 74, 5);
    operator(&mut v, 3, [99, 36, 25, 35], [99, 65, 0, 0], (2, 0), 80, 2);
    operator(&mut v, 4, [99, 45, 30, 40], [99, 50, 0, 0], (5, 0), 62, 5);
    operator(&mut v, 5, [85, 25, 20, 30], [99, 80, 0, 0], (0, 0), 70, 1);
    operator(&mut v, 6, [90, 40, 30, 40], [99, 40, 0, 0], (1, 0), 40, 3);
    v.set_op(3, op::DETUNE, 9);
    v.set_name("GLASS BELL");
    v
}

/// A plucked bass (algorithm 1): a half-ratio body bitten by its
/// modulator's quick decay, under a stack that adds a woody knock.
fn wood_bass() -> Voice {
    use super::dx7::global;
    let mut v = Voice::init();
    v.0[global::ALGORITHM] = 0;
    v.0[global::FEEDBACK] = 4;
    v.0[global::TRANSPOSE] = 24;
    operator(&mut v, 1, [99, 40, 30, 70], [99, 82, 0, 0], (0, 0), 99, 2);
    operator(&mut v, 2, [99, 62, 40, 70], [99, 55, 0, 0], (0, 0), 82, 6);
    operator(&mut v, 3, [99, 75, 50, 70], [99, 0, 0, 0], (1, 0), 72, 3);
    operator(&mut v, 4, [99, 80, 60, 70], [99, 0, 0, 0], (3, 0), 70, 5);
    operator(&mut v, 5, [99, 85, 60, 70], [99, 0, 0, 0], (5, 0), 50, 4);
    operator(&mut v, 6, [99, 90, 60, 70], [99, 0, 0, 0], (1, 0), 45, 2);
    v.set_name("WOOD BASS");
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_node_has_the_seeds() {
        let garden = Garden::of(&BTreeMap::new());
        assert_eq!(garden.voices.len(), 4);
        assert_eq!(garden.playing().voice.name(), "SOFT KEYS");
        assert!(seeds().iter().all(Voice::in_range));
        let names: Vec<String> = seeds().iter().map(Voice::name).collect();
        assert_eq!(names, ["SOFT KEYS", "GLASS BELL", "WOOD BASS", "INIT VOICE"]);
    }

    #[test]
    fn a_garden_is_written_and_read_back() {
        let mut garden = Garden::seeded();
        garden.mutate(0.5, 1).unwrap();
        garden.pick(1);
        garden.cross(2).unwrap();
        garden.keep(0, true);
        let labels: BTreeMap<String, String> = garden.labels().into_iter().collect();
        assert_eq!(Garden::of(&labels), garden);
    }

    #[test]
    fn a_mutant_plays_and_remembers_its_parent() {
        let mut garden = Garden::seeded();
        let child = garden.mutate(0.4, 3).unwrap();
        assert_eq!(garden.current, child);
        let grown = garden.get(child).unwrap();
        assert_eq!(grown.parents, vec![0]);
        assert_eq!(grown.generation, 1);
        assert_eq!(grown.voice.name(), "SOFT KEY 1");
        assert_eq!(garden.lineage(child), "Mutant of SOFT KEYS (1)");
        let grandchild = garden.mutate(0.4, 4).unwrap();
        assert_eq!(garden.get(grandchild).unwrap().voice.name(), "SOFT KEY 2");
    }

    #[test]
    fn a_cross_needs_a_partner() {
        let mut garden = Garden::seeded();
        assert_eq!(garden.cross(1), None);
        garden.pick(0);
        assert_eq!(garden.partner, None, "not the playing voice itself");
        garden.pick(1);
        let child = garden.cross(1).unwrap();
        assert_eq!(garden.get(child).unwrap().parents, vec![0, 1]);
        assert_eq!(garden.get(child).unwrap().voice.name(), "SOFT BELL");
        assert_eq!(garden.partner, Some(1), "still picked, to cross again");
    }

    #[test]
    fn full_it_weeds_the_oldest_it_may() {
        let mut garden = Garden::seeded();
        garden.keep(1, true);
        while garden.voices.len() < ROOM {
            garden.mutate(0.3, garden.next as u64).unwrap();
        }
        garden.recall(0);
        let ids: Vec<u32> = garden.voices.iter().map(|g| g.id).collect();
        let child = garden.mutate(0.3, 99).unwrap();
        assert_eq!(garden.voices.len(), ROOM);
        assert!(garden.get(0).is_some(), "the parent was playing");
        assert!(garden.get(1).is_some(), "kept");
        assert!(garden.get(2).is_none(), "the oldest it may weed");
        assert_eq!(garden.place(child), Some(2), "in the weeded voice's place");
        assert_eq!(garden.voices.iter().map(|g| g.id).filter(|id| ids.contains(id)).count(), ROOM - 1);
        for g in &mut garden.voices {
            g.kept = true;
        }
        assert!(!garden.has_room());
        assert_eq!(garden.mutate(0.3, 100), None, "full of kept voices");
    }

    #[test]
    fn a_file_plants_as_many_as_fit() {
        let mut garden = Garden::seeded();
        let bank: Vec<Voice> = (0..32).map(|k| super::super::breeding::mutate(&Voice::init(), 1.0, k)).collect();
        assert_eq!(garden.plant(&bank), 32, "the seeds weeded to make room");
        assert_eq!(garden.bank(), bank, "in the file's order");
        assert_eq!(garden.playing().origin, Origin::Loaded);
        assert_eq!(garden.voices.len(), ROOM);
        let mut garden = Garden::seeded();
        garden.keep(2, true);
        assert_eq!(garden.plant(&bank), 31, "room for all but the kept one");
        assert_eq!(garden.voices[0].voice.name(), "WOOD BASS");
        assert_eq!(garden.playing().voice, bank[0]);
    }

    #[test]
    fn a_garden_that_wont_read_is_the_seeds() {
        let labels = BTreeMap::from([(KEY.to_string(), "{not json".to_string())]);
        assert_eq!(Garden::of(&labels), Garden::seeded());
    }

    #[test]
    fn brightness_follows_the_modulators() {
        let soft = brightness(&soft_keys());
        let mut harsh = soft_keys();
        for n in [2, 4, 6] {
            harsh.set_op(n, op::LEVEL, 99);
        }
        assert!(brightness(&harsh) > soft);
        assert!(brightness(&Voice::init()) < soft);
    }
}
