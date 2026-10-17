//! Chord Sequencer module.
//!
//! Sixteen steps, each a chord: a root, a chord type and an optional slash
//! bass. Clocked like the Step Sequencer, it plays each chord as polyphonic
//! pitch, gate and velocity, one channel per chord tone, so a pad or an
//! electric piano made of polyphonic modules plays a progression on its own.
//!
//! The chord tones are placed by a voicing (close, open or spread) in the
//! octave Range names. With Voice Leading on, each chord takes the inversion
//! nearest the one before, so the voices move as little as they can, the way
//! a keyboard player's hands do.
//!
//! With Strum up, the voices don't all start on the clock: each channel's
//! gate rises a little after the one before, up or down the chord, as a
//! guitarist's pick or a harpist's fingers sound it. Strum Taper weights
//! the strings struck later, softer or louder, and a step can name its own
//! stroke, up or down, over Strum Dir.
//!
//! A chord being played into its face can be heard on its outputs (see
//! [`DspModule::audition`]): it sounds for as long as the hand is down,
//! clock or no clock, and the sequence comes back on its next chord.
//!
//! It holds four patterns, A to D, played in the order a Chain gives, or
//! picked by a Pattern CV, as on the Step Sequencer: a 16-bar verse in A, a
//! chorus in B. Pattern A's parameters are the ones the sequencer always
//! had ("Step 3 Root"); B to D's come after everything else ("Step B3
//! Root"), so patches saved before there were patterns load with only A.

use std::sync::LazyLock;

use crate::dsp::{
    context::ProcessContext,
    gate::{Edge, Pulse, GATE_THRESHOLD},
    module_trait::{Audition, DspModule, ModuleCategory, ModuleInfo, Readout},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{connected_input, SignalBuffer, MAX_CHANNELS},
    ParameterDisplay, SignalType,
};

use super::sequencer::{PatternPosition, SequenceDirection, StepTimer};
use super::trigger_sequencer::{chain_from, TriggerSequencer, CHAIN_CHOICES, CHAIN_IDS, CHAIN_NAMES, CHAIN_SLOTS, PATTERNS, PATTERN_NAMES};

pub mod recognition;

/// Maximum number of steps in the sequencer.
pub const MAX_STEPS: usize = 16;

/// The most notes a chord voicing holds: one per channel of a cable.
pub const MAX_VOICES: usize = MAX_CHANNELS;

/// Per-step parameter names and ids, "Step 3 Root" and "step_3_root".
macro_rules! per_step {
    ($name:literal, $id:literal) => {
        (
            [
                concat!("Step 1 ", $name), concat!("Step 2 ", $name), concat!("Step 3 ", $name), concat!("Step 4 ", $name),
                concat!("Step 5 ", $name), concat!("Step 6 ", $name), concat!("Step 7 ", $name), concat!("Step 8 ", $name),
                concat!("Step 9 ", $name), concat!("Step 10 ", $name), concat!("Step 11 ", $name), concat!("Step 12 ", $name),
                concat!("Step 13 ", $name), concat!("Step 14 ", $name), concat!("Step 15 ", $name), concat!("Step 16 ", $name),
            ],
            [
                concat!("step_1_", $id), concat!("step_2_", $id), concat!("step_3_", $id), concat!("step_4_", $id),
                concat!("step_5_", $id), concat!("step_6_", $id), concat!("step_7_", $id), concat!("step_8_", $id),
                concat!("step_9_", $id), concat!("step_10_", $id), concat!("step_11_", $id), concat!("step_12_", $id),
                concat!("step_13_", $id), concat!("step_14_", $id), concat!("step_15_", $id), concat!("step_16_", $id),
            ],
        )
    };
}

type StepNames = ([&'static str; MAX_STEPS], [&'static str; MAX_STEPS]);
static ROOT: StepNames = per_step!("Root", "root");
static TYPE: StepNames = per_step!("Type", "type");
static BASS: StepNames = per_step!("Bass", "bass");
static GATE: StepNames = per_step!("Gate", "gate");
static VELOCITY: StepNames = per_step!("Velocity", "velocity");
static TIE: StepNames = per_step!("Tie", "tie");
static TOP: StepNames = per_step!("Top", "top");
static STRUM: StepNames = per_step!("Strum", "strum");

/// The things each step holds, in the order its parameters are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordField {
    Root = 0,
    Type = 1,
    Bass = 2,
    Gate = 3,
    Velocity = 4,
    Tie = 5,
    Top = 6,
    Strum = 7,
}

impl ChordField {
    /// Every field, as copying a pattern copies them.
    pub const ALL: [ChordField; 8] =
        [ChordField::Root, ChordField::Type, ChordField::Bass, ChordField::Gate, ChordField::Velocity, ChordField::Tie, ChordField::Top, ChordField::Strum];

    /// Pattern A's names and ids for this field, a step at a time.
    fn names(self) -> &'static StepNames {
        match self {
            ChordField::Root => &ROOT,
            ChordField::Type => &TYPE,
            ChordField::Bass => &BASS,
            ChordField::Gate => &GATE,
            ChordField::Velocity => &VELOCITY,
            ChordField::Tie => &TIE,
            ChordField::Top => &TOP,
            ChordField::Strum => &STRUM,
        }
    }
}

/// Patterns B to D's parameter names and ids ("Step B3 Root",
/// "step_b3_root"), made once and kept: they must be `'static`, and there
/// are 384 of them. In the order the parameters are: pattern, then step,
/// then field.
static PATTERN_PARAM_NAMES: LazyLock<(Vec<&'static str>, Vec<&'static str>)> = LazyLock::new(|| {
    let keep = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    let (mut names, mut ids) = (Vec::new(), Vec::new());
    for letter in &PATTERN_NAMES[1..] {
        for step in 1..=MAX_STEPS {
            for field in ChordField::ALL {
                let field = field.names().0[0].trim_start_matches("Step 1 ");
                names.push(keep(format!("Step {letter}{step} {field}")));
                ids.push(keep(format!("step_{}{step}_{}", letter.to_lowercase(), field.to_lowercase())));
            }
        }
    }
    (names, ids)
});

/// One of a step's parameters, in any pattern. Every pattern starts from
/// the same progression: Cmaj7, Am7, Fmaj7, G7, the bass walking down from
/// C2, and the rest C major.
fn step_parameter(id: &'static str, name: &'static str, field: ChordField, step: usize) -> ParameterDefinition {
    let starts: [(f32, usize); 4] = [(36.0, 3), (33.0, 4), (29.0, 3), (31.0, 2)];
    let (root, kind) = starts.get(step).copied().unwrap_or((36.0, 0));
    match field {
        ChordField::Root => ParameterDefinition::new(id, name, 0.0, 127.0, root, ParameterDisplay::stepped(""))
            .describe("The chord's root as a MIDI note; the Root output plays it in this octave"),
        ChordField::Type => ParameterDefinition::choice(id, name, &TYPE_LABELS, kind).describe("The kind of chord"),
        ChordField::Bass => {
            ParameterDefinition::choice(id, name, &BASS_LABELS, 0).describe("A note to put under the chord, as in D/F#, or the root")
        }
        ChordField::Gate => ParameterDefinition::toggle(id, name, true).describe("Plays this chord when on; a rest when off"),
        ChordField::Velocity => ParameterDefinition::new(id, name, 0.0, 127.0, 100.0, ParameterDisplay::stepped(""))
            .describe("Velocity for this chord, 0 to 127"),
        ChordField::Tie => ParameterDefinition::toggle(id, name, false)
            .describe("Holds this chord into the next step, which changes to its own chord without a new attack"),
        ChordField::Top => ParameterDefinition::choice(id, name, &TOP_LABELS, 0)
            .describe("A chord tone to put on top of the voicing, as an arranger writes a pad's top line; Auto leaves it to the voicing"),
        ChordField::Strum => ParameterDefinition::choice(id, name, &["Auto", "Up", "Down"], 0)
            .describe("Which way this chord is strummed: from the lowest note (Up) or the highest (Down); Auto follows Strum Dir"),
    }
}

/// A kind of chord: its name and its tones.
#[derive(Debug)]
pub struct ChordType {
    /// What the step's dropdown calls it.
    pub label: &'static str,
    /// What follows the root in a chord's name: "m7" in "Am7".
    pub suffix: &'static str,
    /// Semitones above the root, most important first. A voicing with fewer
    /// voices than tones keeps the first ones, so the 5th, which colours a
    /// chord least, goes first. 9ths, 11ths and 13ths sit above the octave.
    pub tones: &'static [i32],
}

const fn chord(label: &'static str, suffix: &'static str, tones: &'static [i32]) -> ChordType {
    ChordType { label, suffix, tones }
}

/// Every chord type, in the order the Type dropdown lists them.
///
/// The 11 leaves out the 3rd, which clashes with the 11th a half step
/// above it: played that way it's the soft, suspended 11 of soul and jazz
/// (C11 is a B♭ triad over C). The 13 leaves out the 11th for the same
/// reason. Both are how keyboard players voice them.
///
/// New types go on the end: a patch stores a step's type by its place here.
pub static CHORD_TYPES: [ChordType; 19] = [
    chord("maj", "", &[0, 4, 7]),
    chord("min", "m", &[0, 3, 7]),
    chord("7", "7", &[0, 4, 10, 7]),
    chord("maj7", "maj7", &[0, 4, 11, 7]),
    chord("m7", "m7", &[0, 3, 10, 7]),
    chord("m7b5", "m7b5", &[0, 3, 6, 10]),
    chord("dim", "dim", &[0, 3, 6]),
    chord("aug", "aug", &[0, 4, 8]),
    chord("sus2", "sus2", &[0, 2, 7]),
    chord("sus4", "sus4", &[0, 5, 7]),
    chord("6", "6", &[0, 4, 9, 7]),
    chord("m6", "m6", &[0, 3, 9, 7]),
    chord("add9", "add9", &[0, 4, 14, 7]),
    chord("maj9", "maj9", &[0, 4, 11, 14, 7]),
    chord("m9", "m9", &[0, 3, 10, 14, 7]),
    chord("9", "9", &[0, 4, 10, 14, 7]),
    chord("11", "11", &[0, 10, 14, 17, 7]),
    chord("13", "13", &[0, 4, 10, 21, 14, 7]),
    chord("madd9", "madd9", &[0, 3, 14, 7]),
];

/// The Type dropdown's choices.
static TYPE_LABELS: [&str; 19] = [
    "maj", "min", "7", "maj7", "m7", "m7b5", "dim", "aug", "sus2", "sus4", "6", "m6", "add9", "maj9", "m9", "9", "11", "13", "madd9",
];

/// Note names as a lead sheet spells them: flats, except F#.
pub static NOTE_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The Bass dropdown: the root itself, or a note to put under the chord.
static BASS_LABELS: [&str; 13] = ["Root", "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The Top dropdown: left to the voicing, or a chord tone to put on top.
static TOP_LABELS: [&str; 13] = ["Auto", "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// How a chord's tones are spread over the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voicing {
    /// Packed inside an octave, as a hand plays a chord.
    Close = 0,
    /// Drop-2: the second voice from the top dropped an octave, as a jazz
    /// guitarist or a horn section voices a chord.
    Open = 1,
    /// Every other voice raised an octave, for pads and strings.
    Spread = 2,
}

impl Voicing {
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Voicing::Open,
            2 => Voicing::Spread,
            _ => Voicing::Close,
        }
    }
}

/// The order a strum sounds a chord's channels in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrumDirection {
    /// Lowest channel first, as a guitar's downstroke from the bass string.
    Up = 0,
    /// Highest channel first.
    Down = 1,
    /// Up on one clock and down on the next, as a rhythm guitarist's hand
    /// moves down and up in time, striking or not.
    Alternate = 2,
}

impl StrumDirection {
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => StrumDirection::Down,
            2 => StrumDirection::Alternate,
            _ => StrumDirection::Up,
        }
    }

    /// A step's own stroke, from its Strum parameter: Up or Down, or `None`
    /// for Auto, which leaves it to Strum Dir.
    pub fn from_step_param(value: f32) -> Option<Self> {
        match value.round() as i32 {
            1 => Some(StrumDirection::Up),
            2 => Some(StrumDirection::Down),
            _ => None,
        }
    }
}

/// How hard the string struck `turn`th of `strings` is played, as a share
/// of the step's velocity, with Strum Taper at `taper` (-1 to 1). Above 0
/// the strings struck later are softer, as a pick digs into the first and
/// brushes the last, down to nothing at 1. Below 0 they're louder.
pub fn taper_scale(turn: usize, strings: usize, taper: f32) -> f32 {
    let along = if strings > 1 { turn.min(strings - 1) as f32 / (strings - 1) as f32 } else { 0.0 };
    let taper = taper.clamp(-1.0, 1.0);
    if taper >= 0.0 {
        1.0 - taper * along
    } else {
        1.0 + taper * (1.0 - along)
    }
}

/// A channel's place in an audition's strum: counted from the lowest
/// channel up, or from the highest down. A channel past the chord's last
/// note plays that note, so it shares its turn.
fn audition_turn(audition: &Audition, channel: usize) -> usize {
    let last = audition.notes().len().saturating_sub(1);
    let channel = channel.min(last);
    if audition.down {
        last - channel
    } else {
        channel
    }
}

/// A chord's notes, lowest first, as MIDI note numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chord {
    pub notes: [i32; MAX_VOICES],
    pub len: usize,
}

impl Chord {
    pub fn notes(&self) -> &[i32] {
        &self.notes[..self.len]
    }
}

/// One step's chord, as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChordSpec {
    /// MIDI note number: the Root output plays it in this octave.
    pub root: i32,
    /// Index into [`CHORD_TYPES`].
    pub kind: usize,
    /// A pitch class (0 = C) to put under the chord, or `None` for the root.
    pub bass: Option<i32>,
    /// A pitch class (0 = C) the voicing puts on top, or `None` to leave it
    /// to the voicing. Ignored when the voicing can't: when it isn't one of
    /// the tones the voicing keeps.
    pub top: Option<i32>,
}

impl ChordSpec {
    /// The bass note: the slash note, at or below the root, or the root.
    pub fn bass_note(&self) -> i32 {
        match self.bass {
            Some(pc) => self.root - (self.root - pc).rem_euclid(12),
            None => self.root,
        }
    }

    /// The chord's name, as "Am9" or "D/F#".
    pub fn name(&self) -> String {
        let mut name = format!("{}{}", self.root_name(), CHORD_TYPES[self.kind.min(CHORD_TYPES.len() - 1)].suffix);
        if let Some(bass) = self.bass_name() {
            name.push('/');
            name.push_str(&bass);
        }
        name
    }

    /// The root's name, without an octave.
    pub fn root_name(&self) -> &'static str {
        NOTE_NAMES[self.root.rem_euclid(12) as usize]
    }

    /// The slash note's name, spelled as the degree it is above the root:
    /// a major 3rd above D is F#, not Gb, and the note under Am a half step
    /// below its root is G#, as a line cliché spells it.
    pub fn bass_name(&self) -> Option<String> {
        Some(self.spell(self.bass?))
    }

    /// A pitch class (0 = C) spelled as the degree it is above the root,
    /// the way [`Self::bass_name`] spells a slash note.
    pub fn spell(&self, pitch_class: i32) -> String {
        spell_above(self.root_name(), (pitch_class - self.root).rem_euclid(12))
    }
}

/// The note `interval` semitones above the note named `root`, spelled as a
/// scale degree of it. Falls back to the plain spelling rather than write a
/// double sharp or flat.
fn spell_above(root: &str, interval: i32) -> String {
    const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];
    const NATURAL: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    // Which degree each interval is: a minor 2nd is a 2nd, a tritone a 4th
    const DEGREE: [usize; 12] = [0, 1, 1, 2, 2, 3, 3, 4, 5, 5, 6, 6];

    let letter = root.chars().next().unwrap_or('C');
    let root_index = LETTERS.iter().position(|&l| l == letter).unwrap_or(0);
    let root_pc = NOTE_NAMES.iter().position(|&n| n == root).unwrap_or(0) as i32;
    let target = (root_pc + interval).rem_euclid(12);

    let index = (root_index + DEGREE[interval as usize]) % 7;
    let offset = (target - NATURAL[index] + 6).rem_euclid(12) - 6;
    match offset {
        0 => LETTERS[index].to_string(),
        1 => format!("{}#", LETTERS[index]),
        -1 => format!("{}b", LETTERS[index]),
        _ => NOTE_NAMES[target as usize].to_string(),
    }
}

/// How a step's chord is voiced: the global settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoicingSettings {
    pub voicing: Voicing,
    /// How many notes the chord gets. A bass voice, if on, is not one of them.
    pub voices: usize,
    /// The octave the chord's lowest note sits in (4 is middle C's).
    pub range: i32,
    /// The bass plays the root, so a short voicing drops it first.
    pub rootless: bool,
}

/// The tones a voicing of a chord keeps, as pitch classes (0 = C), in the
/// order they stack up from the root as the chord is spelled: 3rd, 5th, 7th,
/// 9th.
pub fn kept_tones(spec: &ChordSpec, settings: &VoicingSettings) -> Vec<i32> {
    let kind = &CHORD_TYPES[spec.kind.min(CHORD_TYPES.len() - 1)];
    let (classes, k, _) = pitch_classes(kind, settings.voices.clamp(1, MAX_VOICES), settings.rootless);
    let mut tones: Vec<i32> = kind.tones.iter().copied().filter(|t| classes[..k].contains(&t.rem_euclid(12))).collect();
    tones.sort_unstable();
    tones.iter().map(|t| (spec.root + t).rem_euclid(12)).collect()
}

/// The chord's pitch classes above its root (0..12), lowest first, after
/// keeping as many tones as there are voices, and which of them the chord
/// is built up from in root position: the root, or without one, the most
/// important tone left (the 3rd, mostly).
fn pitch_classes(kind: &ChordType, voices: usize, rootless: bool) -> ([i32; MAX_VOICES], usize, usize) {
    let tones = kind.tones;
    let count = voices.clamp(1, tones.len());
    let mut kept = [0i32; MAX_VOICES];
    let mut len = 0;
    // Rootless: the root goes last, and only plays if there's room
    let order = tones.iter().skip(rootless as usize).chain(tones.iter().take(rootless as usize));
    for &tone in order.take(count) {
        kept[len] = tone.rem_euclid(12);
        len += 1;
    }
    let bottom = kept[0];
    kept[..len].sort_unstable();
    let home = kept[..len].iter().position(|&pc| pc == bottom).unwrap_or(0);
    (kept, len, home)
}

/// A voicing built up from pitch class `start`: the chord's pitch classes
/// stacked upward, `voices` notes, over and over an octave up when there are
/// more voices than tones. Then opened or spread. Relative to the root's
/// pitch class, lowest first.
fn stack(classes: &[i32], start: usize, voices: usize, voicing: Voicing) -> Chord {
    let mut chord = Chord { len: voices, ..Chord::default() };
    let k = classes.len();
    for i in 0..voices {
        let index = start + i;
        chord.notes[i] = classes[index % k] + 12 * (index / k) as i32;
    }
    match voicing {
        Voicing::Close => {}
        Voicing::Open if voices >= 3 => chord.notes[voices - 2] -= 12,
        Voicing::Open => {}
        Voicing::Spread => {
            for note in chord.notes[..voices].iter_mut().skip(1).step_by(2) {
                *note += 12;
            }
        }
    }
    chord.notes[..voices].sort_unstable();
    chord
}

/// The chord moved by octaves so its lowest note is the first at or above
/// `floor`.
fn placed(mut chord: Chord, floor: i32) -> Chord {
    let lowest = chord.notes[0];
    let shift = floor + (lowest - floor).rem_euclid(12) - lowest;
    for note in &mut chord.notes[..chord.len] {
        *note += shift;
    }
    chord
}

/// How far the voices move from `from` to `to`, channel by channel: the
/// total, then the largest single move.
fn movement(from: &Chord, to: &Chord) -> (i32, i32) {
    let pairs = from.notes().iter().zip(to.notes());
    let total = pairs.clone().map(|(a, b)| (a - b).abs()).sum();
    let largest = pairs.map(|(a, b)| (a - b).abs()).max().unwrap_or(0);
    (total, largest)
}

/// The notes of a chord, voiced.
///
/// Without a chord before it (or with Voice Leading off, which passes
/// `None`), the chord is in root position with its lowest note in the Range
/// octave. After one, it takes whichever inversion moves the voices least
/// from it, with its lowest note within a fifth of the Range octave, so a
/// long progression can't wander off the keyboard.
///
/// A step's Top narrows the choice to the inversions with that tone on top,
/// as an arranger writes a pad's top line. In root position's place, it
/// takes the one of them with its lowest note in the Range octave.
///
/// A semitone the lowest note strays outside the Range octave costs as much
/// as a semitone of movement. Without that pull, the cheapest move each
/// time can carry a looping progression up to the edge of its range and
/// keep it there; with it, the voicing settles back where Range put it.
pub fn voice_chord(spec: &ChordSpec, settings: &VoicingSettings, previous: Option<&Chord>) -> Chord {
    let kind = &CHORD_TYPES[spec.kind.min(CHORD_TYPES.len() - 1)];
    let voices = settings.voices.clamp(1, MAX_VOICES);
    let (classes, k, home) = pitch_classes(kind, voices, settings.rootless);
    let classes = &classes[..k];
    let root_pc = spec.root.rem_euclid(12);
    let floor = 12 * (settings.range + 1);
    let absolute = |mut chord: Chord| {
        for note in &mut chord.notes[..chord.len] {
            *note += root_pc;
        }
        chord
    };

    let build = |start: usize, floor: i32| placed(absolute(stack(classes, start, voices, settings.voicing)), floor);
    // Only inversions with the Top tone on top, if any of them has it there
    let topped = |chord: &Chord| spec.top.is_none_or(|pc| chord.notes[chord.len - 1].rem_euclid(12) == pc);
    let topped_home = std::iter::once(home).chain(0..k).map(|start| build(start, floor)).find(topped);
    let fits = |chord: &Chord| topped_home.is_none() || topped(chord);

    let home = topped_home.unwrap_or_else(|| build(home, floor));
    let Some(previous) = previous.filter(|p| p.len == voices) else {
        return home;
    };

    // How far a chord's lowest note is outside the Range octave
    let stray = |chord: &Chord| (floor - chord.notes[0]).max(chord.notes[0] - (floor + 11)).max(0);
    // The cheapest move, counting the stray, then the smallest largest step,
    // then the least stray
    let cost_of = |chord: &Chord| {
        let (total, largest) = movement(previous, chord);
        (total + stray(chord), largest, stray(chord))
    };
    let mut best = home;
    let mut best_cost = cost_of(&home);
    for start in 0..k {
        let inversion = build(start, floor - 5);
        // Each inversion twice, an octave apart, while its lowest note stays
        // within a fifth either side of the Range octave
        for octave in [0, 12] {
            let mut candidate = inversion;
            for note in &mut candidate.notes[..voices] {
                *note += octave;
            }
            if candidate.notes[0] >= floor + 12 + 5 || !fits(&candidate) {
                continue;
            }
            let cost = cost_of(&candidate);
            if cost < best_cost {
                best = candidate;
                best_cost = cost;
            }
        }
    }
    best
}

/// How many times round a looping pattern is played, at most, before its
/// voicings are taken to have settled.
const SETTLE_LOOPS: usize = 8;

/// Each step's chord as Voice Leading voices it in a pattern left looping:
/// the steps played in `order`, rests (`gates` off) skipped, each chord led
/// from the one before it. The first time round starts from root position,
/// as after a reset, and the pattern goes round again until it plays the
/// same voicings twice. `None` for a rest, or a step not in `order`.
pub fn looped_voicings(specs: &[ChordSpec], gates: &[bool], order: &[usize], settings: &VoicingSettings) -> [Option<Chord>; MAX_STEPS] {
    let mut voiced = [None; MAX_STEPS];
    let mut before: Option<Chord> = None;
    for _ in 0..SETTLE_LOOPS {
        let last = voiced;
        for &step in order.iter().filter(|&&step| gates[step]) {
            let chord = voice_chord(&specs[step], settings, before.as_ref());
            voiced[step] = Some(chord);
            before = Some(chord);
        }
        if voiced == last {
            break;
        }
    }
    voiced
}

/// The step a chord's gate is a share of, in seconds, before the clock has
/// been measured: a beat at 120 BPM.
const GUESSED_STEP: f32 = 0.5;

/// The longest a held chord lasts before the clock has been measured, in
/// seconds: a beat at 15 BPM. Any slower clock is taken to have stopped.
const UNMEASURED_HOLD: f32 = 4.0;

/// A chord sequencer with 16 steps.
///
/// # Ports
///
/// **Inputs:** Clock, Reset and Run, as on the Step Sequencer.
///
/// **Outputs:**
/// - **Pitch**, **Gate**, **Velocity** (polyphonic): one channel per note of
///   the chord, lowest first.
/// - **Root**, **Bass** (mono): the chord's root, and its slash bass (the root
///   when there is none), as V/Oct.
/// - **EOC**: a pulse at the end of the pattern.
///
/// A rest leaves the last chord's pitches where they were, so a release
/// fades on the notes it started on.
///
/// A strum reaches each channel in turn. Until it does, the channel goes on
/// playing the chord before, as a string rings until it's struck again.
/// Then its pitch moves and its gate rises, the pitch never after the gate,
/// and its velocity becomes the new step's, tapered by its place in the
/// strum.
pub struct ChordSequencer {
    current_step: usize,
    ping_pong_direction: i32,
    reset_pending: bool,
    clock: Edge,
    reset: Edge,
    /// Each channel's gate: samples left, and whether it was high last sample.
    gate_timers: [usize; MAX_VOICES],
    gate_high: [bool; MAX_VOICES],
    /// How long a struck channel's gate lasts, for the chord being strummed.
    gate_len: usize,
    /// Samples until the strum reaches each channel, or `None` once it has.
    strum_wait: [Option<usize>; MAX_VOICES],
    /// What each channel plays until the strum reaches it.
    held: [i32; MAX_VOICES],
    /// The chord being strummed was tied into, so it moves the notes
    /// without striking them.
    legato: bool,
    /// Clocks since a reset: Alt strums down on the odd ones.
    strokes: u32,
    /// The Velocity parameter of the step being strummed, and whether
    /// from the top down.
    stroke_velocity: usize,
    stroke_down: bool,
    /// The Velocity parameter of the step that last struck each channel,
    /// and the channel's place in that strum, which its velocity is read from.
    struck: [(usize, usize); MAX_VOICES],
    tied: bool,
    timer: StepTimer,
    /// The end-of-cycle trigger.
    eoc: Pulse,
    random_state: u32,
    sample_rate: f32,
    /// The pattern playing.
    pattern: usize,
    /// Which Chain entry is playing.
    chain_slot: usize,
    /// Clocks into this pass through the pattern (random order counts them).
    pass_clocks: usize,
    /// The pattern the next pass will play, for the face.
    next_pattern: usize,
    /// Pattern was patched in the last block.
    pattern_cv: bool,
    /// The chord sounding, its root and bass, and what it was voiced from.
    sounding: Sounding,
    /// The chord before it, which voice leading moves from.
    before: Option<Chord>,
    /// A chord played from the face: sounding while `auditioning`, and
    /// after that holding its pitches through the release until the
    /// sequence strikes a chord of its own.
    audition: Option<Audition>,
    auditioning: bool,
    /// Samples since the audition was struck. It's strummed as a step is:
    /// each channel is struck `audition_spacing` after the one before, its
    /// gate low for that sample so an envelope already open starts again.
    /// Until then the channel rings on what the sequence had on it.
    audition_age: usize,
    audition_spacing: usize,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

/// What a chord step put on the outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sounding {
    /// The step and settings it was voiced from, so an edit re-voices it.
    spec: ChordSpec,
    settings: VoicingSettings,
    bass_voice: bool,
    /// The voicing, without the bass voice.
    chord: Chord,
    /// Every channel's note, the bass voice first if on.
    channels: Chord,
}

impl ChordSequencer {
    pub fn new() -> Self {
        let ports = vec![
            PortDefinition::input_with_default("clock", "Clock", SignalType::Gate, 0.0).describe("Each rising edge advances to the next chord; patch a Clock gate here"),
            PortDefinition::input_with_default("reset", "Reset", SignalType::Gate, 0.0).describe("A rising edge jumps back to step 1"),
            PortDefinition::input_with_default("run", "Run", SignalType::Gate, 1.0).describe("Steps only advance while high; runs when unpatched"),
            PortDefinition::input_with_default("pattern", "Pattern", SignalType::Control, 0.0).describe("Picks the pattern for each pass in place of the Chain: 0 to 0.25 is A, then B, C and D"),
            PortDefinition::output("pitch", "Pitch", SignalType::Control).describe("Poly V/Oct, one channel per note of the chord, lowest first"),
            PortDefinition::output("gate", "Gate", SignalType::Gate).describe("Poly gate, one channel per note, rising together unless Strum staggers them; patch into a voice's envelope"),
            PortDefinition::output("velocity", "Velocity", SignalType::Control).describe("Poly velocity of the step, 0 to 1, each channel tapered by Strum Taper"),
            PortDefinition::output("root", "Root", SignalType::Control).describe("The chord's root as V/Oct, in the octave the step names: a bass line, or a transpose for an arp"),
            PortDefinition::output("bass", "Bass", SignalType::Control).describe("The slash chord's bass note as V/Oct, or the root when there is none"),
            PortDefinition::output("eoc", "EOC", SignalType::Gate).describe("Short pulse when the sequence reaches its end"),
        ];

        let mut parameters = vec![
            ParameterDefinition::new("steps", "Steps", 1.0, 16.0, 4.0, ParameterDisplay::stepped(""))
                .describe("How many chords each pattern plays before the next pass"),
            ParameterDefinition::choice("direction", "Direction", &["Fwd", "Bwd", "P-P", "Rnd"], 0)
                .describe("Playback order: forward, backward, ping-pong or random"),
            ParameterDefinition::new("gate_length", "Gate Length", 1.0, 100.0, 100.0, ParameterDisplay::linear("%"))
                .describe("How long each chord is held, as a share of the step; 100% holds it until the next clock"),
            ParameterDefinition::choice("voicing", "Voicing", &["Close", "Open", "Spread"], Voicing::Close as usize)
                .describe("Close packs the chord in an octave; Open drops the second voice from the top an octave (drop-2); Spread raises every other voice an octave"),
            ParameterDefinition::toggle("voice_leading", "Voice Leading", true)
                .describe("Each chord takes the inversion nearest the one before, so the voices move as little as they can"),
            ParameterDefinition::new("voices", "Voices", 1.0, MAX_VOICES as f32, 4.0, ParameterDisplay::stepped(""))
                .describe("How many channels the cables carry: chord tones are left out (the 5th first) or doubled an octave up to fit"),
            ParameterDefinition::new("range", "Range", 1.0, 6.0, 3.0, ParameterDisplay::stepped(""))
                .describe("The octave the chord's lowest note sits in; 4 is middle C's"),
            ParameterDefinition::toggle("bass_voice", "Bass Voice", false)
                .describe("Puts the bass note under the chord as the cables' first channel, so a slash chord sounds on one voice. The chord then leaves out its root first"),
            ParameterDefinition::toggle("rootless", "Rootless", false)
                .describe("The chord leaves out its root first, for a bass playing it from the Root or Bass output. Always so with Bass Voice on"),
            ParameterDefinition::new("strum", "Strum", 0.0, 100.0, 0.0, ParameterDisplay::linear("ms"))
                .describe("Time between one channel's gate and the next, as a strummed guitar or a harp sounds a chord; 0 strikes every note at once"),
            ParameterDefinition::choice("strum_dir", "Strum Dir", &["Up", "Down", "Alt"], StrumDirection::Up as usize)
                .describe("Where the strum starts: the lowest note (Up), the highest (Down), or down and up on alternate clocks (Alt). A step can name its own"),
            ParameterDefinition::new("strum_taper", "Strum Taper", -100.0, 100.0, 0.0, ParameterDisplay::linear("%"))
                .describe("Softens the notes struck later in a strum, down to nothing at 100%, as a pick digs into the first strings and brushes the last; below 0 they grow louder instead"),
        ];

        for step in 0..MAX_STEPS {
            parameters.extend(ChordField::ALL.map(|field| step_parameter(field.names().1[step], field.names().0[step], field, step)));
        }
        // The Chain and patterns B to D, after everything the sequencer had
        // before it had patterns
        for slot in 0..CHAIN_SLOTS {
            parameters.push(
                ParameterDefinition::choice(CHAIN_IDS[slot], CHAIN_NAMES[slot], &CHAIN_CHOICES, usize::from(slot == 0))
                    .describe("A pattern in the chain, which plays its patterns a pass each, in order, then repeats"),
            );
        }
        let (names, ids) = &*PATTERN_PARAM_NAMES;
        for (index, (&name, &id)) in names.iter().zip(ids).enumerate() {
            let field = ChordField::ALL[index % ChordField::ALL.len()];
            parameters.push(step_parameter(id, name, field, index / ChordField::ALL.len() % MAX_STEPS));
        }
        debug_assert_eq!(parameters.len(), Self::PARAM_COUNT);

        Self {
            current_step: 0,
            ping_pong_direction: 1,
            reset_pending: true,
            clock: Edge::LOW,
            reset: Edge::LOW,
            gate_timers: [0; MAX_VOICES],
            gate_high: [false; MAX_VOICES],
            gate_len: 0,
            strum_wait: [None; MAX_VOICES],
            held: [0; MAX_VOICES],
            legato: false,
            strokes: 0,
            stroke_velocity: Self::step_velocity_param(0),
            stroke_down: false,
            struck: std::array::from_fn(|channel| (Self::step_velocity_param(0), channel)),
            tied: false,
            timer: StepTimer::new(),
            eoc: Pulse::IDLE,
            random_state: 12345,
            sample_rate: 44100.0,
            pattern: 0,
            chain_slot: 0,
            pass_clocks: 0,
            next_pattern: 0,
            pattern_cv: false,
            sounding: Sounding::SILENT,
            before: None,
            audition: None,
            auditioning: false,
            audition_age: 0,
            audition_spacing: 0,
            ports,
            parameters,
        }
    }

    const PORT_CLOCK: usize = 0;
    const PORT_RESET: usize = 1;
    const PORT_RUN: usize = 2;
    const PORT_PATTERN: usize = 3;
    pub const PORT_PITCH: usize = 0;
    pub const PORT_GATE: usize = 1;
    pub const PORT_VELOCITY: usize = 2;
    pub const PORT_ROOT: usize = 3;
    pub const PORT_BASS: usize = 4;
    pub const PORT_EOC: usize = 5;

    pub const PARAM_STEPS: usize = 0;
    pub const PARAM_DIRECTION: usize = 1;
    pub const PARAM_GATE_LENGTH: usize = 2;
    pub const PARAM_VOICING: usize = 3;
    pub const PARAM_VOICE_LEADING: usize = 4;
    pub const PARAM_VOICES: usize = 5;
    pub const PARAM_RANGE: usize = 6;
    pub const PARAM_BASS_VOICE: usize = 7;
    pub const PARAM_ROOTLESS: usize = 8;
    pub const PARAM_STRUM: usize = 9;
    pub const PARAM_STRUM_DIR: usize = 10;
    pub const PARAM_STRUM_TAPER: usize = 11;
    const STEP_PARAMS: usize = 12;
    const PER_STEP: usize = 8;

    pub const fn step_root_param(step: usize) -> usize {
        Self::STEP_PARAMS + step * Self::PER_STEP
    }
    pub const fn step_type_param(step: usize) -> usize {
        Self::step_root_param(step) + 1
    }
    pub const fn step_bass_param(step: usize) -> usize {
        Self::step_root_param(step) + 2
    }
    pub const fn step_gate_param(step: usize) -> usize {
        Self::step_root_param(step) + 3
    }
    pub const fn step_velocity_param(step: usize) -> usize {
        Self::step_root_param(step) + 4
    }
    pub const fn step_tie_param(step: usize) -> usize {
        Self::step_root_param(step) + 5
    }
    pub const fn step_top_param(step: usize) -> usize {
        Self::step_root_param(step) + 6
    }
    pub const fn step_strum_param(step: usize) -> usize {
        Self::step_root_param(step) + 7
    }

    /// The first of the eight Chain slots, after pattern A's steps.
    pub const PARAM_CHAIN: usize = Self::STEP_PARAMS + MAX_STEPS * Self::PER_STEP;
    /// Patterns B to D's steps, after the Chain.
    const PARAM_PATTERNS: usize = Self::PARAM_CHAIN + CHAIN_SLOTS;
    pub const PARAM_COUNT: usize = Self::PARAM_PATTERNS + (PATTERNS - 1) * MAX_STEPS * Self::PER_STEP;

    /// The parameter holding one of a step's fields (both 0-based).
    pub const fn step_param(pattern: usize, step: usize, field: ChordField) -> usize {
        let start = if pattern == 0 { Self::STEP_PARAMS } else { Self::PARAM_PATTERNS + (pattern - 1) * MAX_STEPS * Self::PER_STEP };
        start + step * Self::PER_STEP + field as usize
    }

    /// The name of the parameter holding a step's field: "Step 3 Root" in
    /// pattern A, "Step B3 Root" in B.
    pub fn step_param_name(pattern: usize, step: usize, field: ChordField) -> &'static str {
        match pattern {
            0 => field.names().0[step],
            _ => PATTERN_PARAM_NAMES.0[((pattern - 1) * MAX_STEPS + step) * Self::PER_STEP + field as usize],
        }
    }

    /// The patterns the Chain plays, in order, and how many.
    pub fn chain(params: &[f32]) -> ([usize; CHAIN_SLOTS], usize) {
        chain_from(&params[Self::PARAM_CHAIN..Self::PARAM_CHAIN + CHAIN_SLOTS])
    }

    /// The pattern for the Chain slot playing, or for the Pattern CV.
    fn pick(&mut self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => TriggerSequencer::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                self.chain_slot %= len;
                chain[self.chain_slot]
            }
        }
    }

    /// The pattern the next pass will play.
    fn coming_pattern(&self, params: &[f32], cv: Option<f32>) -> usize {
        match cv {
            Some(cv) => TriggerSequencer::pattern_for_cv(cv),
            None => {
                let (chain, len) = Self::chain(params);
                if self.reset_pending { chain[0] } else { chain[(self.chain_slot + 1) % len] }
            }
        }
    }

    /// The readout: the step playing, then each channel's MIDI note (-1
    /// past the last channel).
    pub const READOUT_STEP: usize = 0;
    pub const READOUT_NOTES: usize = 1;
    /// Where it is in its chain, after the notes: the pattern, then the
    /// next, as the Step Sequencer reports them (see [`PatternPosition`]).
    pub const READOUT_PATTERN: usize = Self::READOUT_NOTES + MAX_VOICES;

    /// How long the EOC trigger lasts, in seconds.
    const EOC_SECONDS: f32 = 0.001;

    /// A step's chord in pattern A, as its parameters write it.
    pub fn spec(params: &[f32], step: usize) -> ChordSpec {
        Self::spec_in(params, 0, step)
    }

    /// A step's chord in any pattern, as its parameters write it.
    pub fn spec_in(params: &[f32], pattern: usize, step: usize) -> ChordSpec {
        let field = |field| params[Self::step_param(pattern, step, field)];
        let bass = field(ChordField::Bass).round() as i32;
        let top = field(ChordField::Top).round() as i32;
        ChordSpec {
            root: field(ChordField::Root).round().clamp(0.0, 127.0) as i32,
            kind: (field(ChordField::Type).round().max(0.0) as usize).min(CHORD_TYPES.len() - 1),
            bass: (1..=12).contains(&bass).then(|| bass - 1),
            top: (1..=12).contains(&top).then(|| top - 1),
        }
    }

    /// The global voicing settings, as the parameters set them.
    fn settings(params: &[f32], channels: usize) -> (VoicingSettings, bool) {
        let bass_voice = params[Self::PARAM_BASS_VOICE] > 0.5;
        let voices = channels.saturating_sub(bass_voice as usize).max(1);
        let settings = VoicingSettings {
            voicing: Voicing::from_param(params[Self::PARAM_VOICING]),
            voices,
            range: params[Self::PARAM_RANGE].round() as i32,
            rootless: bass_voice || params[Self::PARAM_ROOTLESS] > 0.5,
        };
        (settings, bass_voice)
    }

    /// Voices a chord into `sounding`, led from the chord before it.
    fn voice(&mut self, params: &[f32], spec: ChordSpec, channels: usize) {
        let (settings, bass_voice) = Self::settings(params, channels);
        let leading = params[Self::PARAM_VOICE_LEADING] > 0.5;
        let chord = voice_chord(&spec, &settings, self.before.as_ref().filter(|_| leading));
        let mut all = Chord { len: channels, ..Chord::default() };
        let bass = bass_voice as usize;
        if bass_voice {
            all.notes[0] = spec.bass_note();
        }
        for (slot, &note) in all.notes[bass..channels].iter_mut().zip(chord.notes()) {
            *slot = note;
        }
        self.sounding = Sounding { spec, settings, bass_voice, chord, channels: all };
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.random_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.random_state = x;
        x
    }

    /// Moves to the next step; true if that ended a cycle.
    fn advance_step(&mut self, num_steps: usize, direction: SequenceDirection) -> bool {
        match direction {
            SequenceDirection::Forward => {
                let end = self.current_step + 1 >= num_steps;
                self.current_step = (self.current_step + 1) % num_steps;
                end
            }
            SequenceDirection::Backward => {
                let end = self.current_step == 0;
                self.current_step = if end { num_steps - 1 } else { self.current_step - 1 };
                end
            }
            SequenceDirection::PingPong => {
                let next = self.current_step as i32 + self.ping_pong_direction;
                if next >= num_steps as i32 {
                    self.ping_pong_direction = -1;
                    self.current_step = num_steps.saturating_sub(2);
                    true
                } else if next < 0 {
                    self.ping_pong_direction = 1;
                    self.current_step = 1.min(num_steps - 1);
                    true
                } else {
                    self.current_step = next as usize;
                    false
                }
            }
            SequenceDirection::Random => {
                self.current_step = (self.next_random() as usize) % num_steps;
                false
            }
        }
    }

    /// How long a new chord's gate stays high, in samples, as on the Step
    /// Sequencer: a share of the coming step, held until the next clock at
    /// 100% or across a tie, and let go after two steps if the clock stops.
    ///
    /// Until two clocks have been seen there's no step to measure. A held
    /// chord then lasts until the next clock (or [`UNMEASURED_HOLD`]), so
    /// the first chord after pressing play is as long as the rest, and a
    /// shorter one is its share of [`GUESSED_STEP`].
    fn gate_samples(&self, gate_length: f32, tied: bool) -> usize {
        let samples = match self.timer.coming_step() {
            Some(step) if tied || gate_length >= 1.0 => 2 * step,
            Some(step) => (step as f32 * gate_length) as usize,
            None if tied || gate_length >= 1.0 => (self.sample_rate * UNMEASURED_HOLD) as usize,
            None => (self.sample_rate * GUESSED_STEP * gate_length) as usize,
        };
        samples.max(1)
    }

    /// Samples between one channel's strike and the next. Once the clock is
    /// measured, a strum fits inside its step, so its last note is struck
    /// before the next chord's first.
    fn strum_spacing(&self, strum_ms: f32, channels: usize) -> usize {
        let spacing = (strum_ms.max(0.0) / 1000.0 * self.sample_rate) as usize;
        match self.timer.coming_step() {
            Some(step) => spacing.min(step / channels.max(1)),
            None => spacing,
        }
    }

    /// The audition on a channel, once its strum has reached it (or the
    /// hand has lifted), and the channel's place in that strum.
    fn auditioned(&self, channel: usize) -> Option<(Audition, usize)> {
        let audition = self.audition.filter(|audition| !audition.notes().is_empty())?;
        let turn = audition_turn(&audition, channel);
        (!self.auditioning || self.audition_age >= turn * self.audition_spacing).then_some((audition, turn))
    }

    /// The note on a channel now: an audition's, or the chord before,
    /// until the strum reaches it.
    fn playing(&self, channel: usize) -> i32 {
        if let Some((audition, _)) = self.auditioned(channel) {
            let notes = audition.notes();
            return notes[channel.min(notes.len() - 1)] as i32;
        }
        match self.strum_wait[channel] {
            Some(_) => self.held[channel],
            None => self.sounding.channels.notes[channel],
        }
    }

    /// How many channels have a note to show.
    fn notes_shown(&self) -> usize {
        match self.audition {
            Some(audition) if self.auditioning => audition.notes().len(),
            _ => self.sounding.channels.len,
        }
    }

    /// The step playing.
    pub fn current_step(&self) -> usize {
        self.current_step
    }

    /// The notes on the cables now, lowest channel first.
    pub fn sounding(&self) -> &[i32] {
        self.sounding.channels.notes()
    }
}

impl Sounding {
    const SILENT: Sounding = Sounding {
        spec: ChordSpec { root: -1, kind: 0, bass: None, top: None },
        settings: VoicingSettings { voicing: Voicing::Close, voices: 0, range: 0, rootless: false },
        bass_voice: false,
        chord: Chord { notes: [0; MAX_VOICES], len: 0 },
        channels: Chord { notes: [0; MAX_VOICES], len: 0 },
    };
}

impl Default for ChordSequencer {
    fn default() -> Self {
        Self::new()
    }
}

fn note_to_voct(note: i32) -> f32 {
    (note as f32 - 60.0) / 12.0
}

impl DspModule for ChordSequencer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "seq.chord",
            name: "Chord Sequencer",
            category: ModuleCategory::Utility,
            description: "16 chords, each a root, a type and a slash bass, played on polyphonic cables with voice leading",
        };
        &INFO
    }

    fn ports(&self) -> &[PortDefinition] {
        &self.ports
    }

    fn parameters(&self) -> &[ParameterDefinition] {
        &self.parameters
    }

    fn prepare(&mut self, sample_rate: f32, _max_block_size: usize) {
        self.sample_rate = sample_rate;
        self.timer.forget();
    }

    fn process(&mut self, inputs: &[&SignalBuffer], outputs: &mut [SignalBuffer], params: &[f32], context: &ProcessContext) {
        let num_steps = (params[Self::PARAM_STEPS].round() as usize).clamp(1, MAX_STEPS);
        let direction = SequenceDirection::from_param(params[Self::PARAM_DIRECTION]);
        let gate_length = params[Self::PARAM_GATE_LENGTH] / 100.0;
        let strum_ms = params[Self::PARAM_STRUM];
        let strum_direction = StrumDirection::from_param(params[Self::PARAM_STRUM_DIR]);
        let taper = params[Self::PARAM_STRUM_TAPER] / 100.0;

        // As many channels as the knob asks for and the cables have room for
        let room = outputs[..Self::PORT_ROOT].iter().map(|output| output.max_channels()).min().unwrap_or(1);
        let channels = (params[Self::PARAM_VOICES].round() as usize).clamp(1, room);
        for output in &mut outputs[..Self::PORT_ROOT] {
            output.set_channels(channels);
        }
        for output in &mut outputs[Self::PORT_ROOT..] {
            output.set_channels(1);
        }
        // A channel the cables no longer carry has no strum to wait for
        for wait in &mut self.strum_wait[channels..] {
            *wait = None;
        }
        // A chord played from the face is strummed as the step will be
        self.audition_spacing = self.strum_spacing(strum_ms, channels);

        // Before the first clock the cables carry the first chord, silent,
        // as a Step Sequencer's Pitch shows its first note: a pad that's
        // always a little open plays it. After that, an edit to the chord
        // playing, or to how chords are voiced, is heard at once, voiced
        // from the same chord before it. A rest keeps the chord it holds
        let (settings, bass_voice) = Self::settings(params, channels);
        let step_spec = Self::spec_in(params, self.pattern, self.current_step);
        let step_on = params[Self::step_param(self.pattern, self.current_step, ChordField::Gate)] > 0.5;
        if self.sounding.channels.len == 0 || step_on && step_spec != self.sounding.spec {
            self.voice(params, step_spec, channels);
        } else if settings != self.sounding.settings || bass_voice != self.sounding.bass_voice {
            self.voice(params, self.sounding.spec, channels);
        }

        let clock_in = inputs.get(Self::PORT_CLOCK);
        let reset_in = inputs.get(Self::PORT_RESET);
        let run_in = inputs.get(Self::PORT_RUN);
        let pattern_in = connected_input(inputs, Self::PORT_PATTERN);
        self.pattern_cv = pattern_in.is_some();

        for i in 0..context.block_size {
            let clock_rising = self.clock.rising(clock_in.map_or(0.0, |buf| buf.at(i)));
            let reset_rising = self.reset.rising(reset_in.map_or(0.0, |buf| buf.at(i)));
            let is_running = run_in.map_or(1.0, |buf| buf.at(i)) > GATE_THRESHOLD;
            let cv = pattern_in.map(|buf| buf.at(i));

            if reset_rising {
                self.current_step = direction.start_step(num_steps);
                self.ping_pong_direction = 1;
                self.reset_pending = true;
                self.gate_timers = [0; MAX_VOICES];
                self.strum_wait = [None; MAX_VOICES];
                self.strokes = 0;
                self.tied = false;
                self.timer.forget_gap();
            }

            if clock_rising {
                self.timer.clock();
            }

            if clock_rising && is_running {
                // The first chord after a reset starts afresh in root
                // position, so a progression that's reset plays the same
                // voicings each time round. It starts the chain's first
                // pattern, or the one the Pattern CV picks
                let fresh = self.reset_pending;
                let end_of_chain = if self.reset_pending {
                    self.reset_pending = false;
                    self.current_step = direction.start_step(num_steps);
                    self.chain_slot = 0;
                    self.pass_clocks = 0;
                    self.pattern = self.pick(params, cv);
                    false
                } else {
                    let hit_end = self.advance_step(num_steps, direction);
                    // A pass through the pattern ends where the sequence
                    // wraps or turns; in random order, after Steps clocks.
                    // Then the Chain's next pattern plays, or the CV's
                    self.pass_clocks += 1;
                    let pass_over = match direction {
                        SequenceDirection::Random => self.pass_clocks >= num_steps,
                        _ => hit_end,
                    };
                    let mut chain_over = false;
                    if pass_over {
                        self.pass_clocks = 0;
                        self.chain_slot += 1;
                        if self.chain_slot >= Self::chain(params).1 {
                            self.chain_slot = 0;
                            chain_over = true;
                        }
                        self.pattern = self.pick(params, cv);
                    }
                    // With the Pattern CV choosing, every pass is the whole
                    // sequence
                    hit_end && (chain_over || cv.is_some())
                };

                // The hand moves on every clock, rests, ties and steps that
                // name their own stroke included, so Alt keeps time as a
                // rhythm guitarist's does
                let step = self.current_step;
                let field = |field| params[Self::step_param(self.pattern, step, field)];
                let down = match StrumDirection::from_step_param(field(ChordField::Strum)).unwrap_or(strum_direction) {
                    StrumDirection::Up => false,
                    StrumDirection::Down => true,
                    StrumDirection::Alternate => self.strokes % 2 == 1,
                };
                self.strokes = self.strokes.wrapping_add(1);

                // A chord that plays is voiced from the one sounding, then
                // strummed: each channel keeps its note until its turn. A rest
                // leaves the pitches alone, so the release ends where it began
                if field(ChordField::Gate) > 0.5 {
                    let tie = field(ChordField::Tie) > 0.5;
                    for channel in 0..channels {
                        self.held[channel] = self.playing(channel);
                    }
                    // An audition let go of rings until now, as a chord before would
                    if !self.auditioning {
                        self.audition = None;
                    }
                    self.legato = self.tied;
                    self.gate_len = self.gate_samples(gate_length, tie);
                    self.tied = tie;
                    self.before = (!fresh && self.sounding.chord.len > 0).then_some(self.sounding.chord);
                    self.voice(params, Self::spec_in(params, self.pattern, step), channels);
                    let spacing = self.strum_spacing(strum_ms, channels);
                    self.stroke_velocity = Self::step_param(self.pattern, step, ChordField::Velocity);
                    self.stroke_down = down;
                    for channel in 0..channels {
                        let turn = if down { channels - 1 - channel } else { channel };
                        self.strum_wait[channel] = Some(turn * spacing);
                    }
                } else {
                    self.gate_timers = [0; MAX_VOICES];
                    self.strum_wait = [None; MAX_VOICES];
                    self.tied = false;
                }

                // EOC once the whole chain has played (never in random
                // order, which has no end)
                if end_of_chain && direction != SequenceDirection::Random {
                    self.eoc.fire(Self::EOC_SECONDS, self.sample_rate);
                }
            }

            if self.current_step >= num_steps {
                self.current_step = 0;
            }

            for channel in 0..channels {
                // The strum reaches this channel: its note moves and, unless
                // the chord was tied into, its gate drops for this sample so
                // the envelope starts again
                let mut retrigger = false;
                if self.strum_wait[channel] == Some(0) {
                    self.strum_wait[channel] = None;
                    retrigger = self.gate_high[channel] && !self.legato;
                    self.gate_timers[channel] = self.gate_len;
                    let turn = if self.stroke_down { channels - 1 - channel } else { channel };
                    self.struck[channel] = (self.stroke_velocity, turn);
                }
                let sequenced = self.gate_timers[channel] > 0 && !retrigger;
                // An audition's gates are held while the hand is down, over
                // whatever the sequence is doing underneath, each from the
                // sample after its strum reaches it
                let gate_active = match self.audition.filter(|_| self.auditioning) {
                    Some(audition) if channel < audition.notes().len() => {
                        let strike = audition_turn(&audition, channel) * self.audition_spacing;
                        match self.audition_age.cmp(&strike) {
                            std::cmp::Ordering::Greater => true,
                            std::cmp::Ordering::Equal => false,
                            std::cmp::Ordering::Less => sequenced,
                        }
                    }
                    Some(_) => false,
                    None => sequenced,
                };
                self.gate_high[channel] = gate_active;
                outputs[Self::PORT_PITCH].channel_mut(channel)[i] = note_to_voct(self.playing(channel));
                outputs[Self::PORT_GATE].channel_mut(channel)[i] = if gate_active { 1.0 } else { 0.0 };
                // A channel plays as hard as the step (or the hand) that last
                // struck it, tapered by its place in that strum
                let velocity = match self.auditioned(channel) {
                    Some((audition, turn)) => audition.velocity * taper_scale(turn, audition.notes().len(), taper),
                    None => {
                        let (velocity, turn) = self.struck[channel];
                        params[velocity] / 127.0 * taper_scale(turn, channels, taper)
                    }
                };
                outputs[Self::PORT_VELOCITY].channel_mut(channel)[i] = velocity;
            }
            if self.auditioning {
                self.audition_age = self.audition_age.saturating_add(1);
            }
            let (root, bass) = match self.audition {
                Some(audition) => (audition.root as i32, audition.bass as i32),
                None => (self.sounding.spec.root, self.sounding.spec.bass_note()),
            };
            outputs[Self::PORT_ROOT].samples[i] = note_to_voct(root);
            outputs[Self::PORT_BASS].samples[i] = note_to_voct(bass);
            outputs[Self::PORT_EOC].samples[i] = self.eoc.sample();

            for timer in &mut self.gate_timers {
                *timer = timer.saturating_sub(1);
            }
            for wait in self.strum_wait.iter_mut().flatten() {
                *wait = wait.saturating_sub(1);
            }
            self.timer.tick();
        }

        let last_cv = pattern_in.and_then(|buf| buf.samples.get(context.block_size.saturating_sub(1)).copied());
        self.next_pattern = self.coming_pattern(params, last_cv);
    }

    fn reset(&mut self) {
        self.current_step = 0;
        self.ping_pong_direction = 1;
        self.reset_pending = true;
        self.clock.reset();
        self.reset.reset();
        self.gate_timers = [0; MAX_VOICES];
        self.gate_high = [false; MAX_VOICES];
        self.gate_len = 0;
        self.strum_wait = [None; MAX_VOICES];
        self.legato = false;
        self.strokes = 0;
        self.stroke_velocity = Self::step_velocity_param(0);
        self.stroke_down = false;
        self.struck = std::array::from_fn(|channel| (Self::step_velocity_param(0), channel));
        self.tied = false;
        self.timer.forget();
        self.eoc.clear();
        self.sounding = Sounding::SILENT;
        self.before = None;
        self.audition = None;
        self.auditioning = false;
        self.audition_age = 0;
        self.pattern = 0;
        self.chain_slot = 0;
        self.pass_clocks = 0;
    }

    fn readout(&self, _params: &[f32]) -> Option<Readout> {
        let mut readout = Readout::default();
        readout.values[Self::READOUT_STEP] = self.current_step as f32;
        let shown = self.notes_shown();
        for (i, value) in readout.values[Self::READOUT_NOTES..Self::READOUT_PATTERN].iter_mut().enumerate() {
            *value = if i < shown { self.playing(i) as f32 } else { -1.0 };
        }
        let position = PatternPosition {
            pattern: self.pattern,
            chain_slot: self.chain_slot,
            started: !self.reset_pending,
            next_pattern: self.next_pattern,
            pattern_cv: self.pattern_cv,
        };
        position.write(&mut readout, Self::READOUT_PATTERN);
        Some(readout)
    }

    /// A chord played in from the face sounds at once, strummed as its step
    /// will be, and is held until the hand lifts. Notes landing under the
    /// same hand move the pitches without striking again. When it lifts,
    /// the sequence's own gates are let go of, so its next chord, not one
    /// struck while the hand was down, is what's heard after.
    fn audition(&mut self, chord: Option<Audition>) {
        match chord.filter(|chord| !chord.notes().is_empty()) {
            Some(chord) => {
                if !self.auditioning {
                    self.audition_age = 0;
                }
                self.audition = Some(chord);
                self.auditioning = true;
            }
            None if self.auditioning => {
                self.auditioning = false;
                self.gate_timers = [0; MAX_VOICES];
                self.strum_wait = [None; MAX_VOICES];
            }
            None => {}
        }
    }

    fn polyphonic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAJ: usize = 0;
    const MIN: usize = 1;
    const DOM7: usize = 2;
    const MAJ7: usize = 3;
    const M7: usize = 4;
    const MAJ9: usize = 13;

    fn kind(label: &str) -> usize {
        CHORD_TYPES.iter().position(|t| t.label == label).unwrap()
    }

    fn spec(root: i32, kind: usize, bass: Option<i32>) -> ChordSpec {
        ChordSpec { root, kind, bass, top: None }
    }

    fn close(voices: usize, range: i32) -> VoicingSettings {
        VoicingSettings { voicing: Voicing::Close, voices, range, rootless: false }
    }

    /// Every parameter at its default, with the given steps' chords written
    /// as (root, type), all gated, no ties.
    fn params_with(chords: &[(i32, usize)]) -> Vec<f32> {
        let mut params: Vec<f32> = ChordSequencer::new().parameters().iter().map(|p| p.default).collect();
        params[ChordSequencer::PARAM_STEPS] = chords.len() as f32;
        for (step, &(root, kind)) in chords.iter().enumerate() {
            params[ChordSequencer::step_root_param(step)] = root as f32;
            params[ChordSequencer::step_type_param(step)] = kind as f32;
        }
        params
    }

    /// Everything the sequencer puts out over a run.
    struct Run {
        /// Each channel's pitch, as MIDI notes, sample by sample.
        notes: Vec<Vec<f32>>,
        gates: Vec<Vec<f32>>,
        velocities: Vec<Vec<f32>>,
        root: Vec<f32>,
        bass: Vec<f32>,
        eoc: Vec<f32>,
        channels: usize,
        mono_channels: usize,
    }

    impl Run {
        /// The chord on the cables at `t`, lowest channel first.
        fn chord(&self, t: usize) -> Vec<i32> {
            (0..self.channels).map(|c| self.notes[c][t] as i32).collect()
        }
    }

    /// Runs the sequencer at 1 kHz for `ms`, with a 1 ms clock pulse at each
    /// time in `clocks`, `block` samples at a time.
    fn run_with(seq: &mut ChordSequencer, params: &[f32], ms: usize, clocks: &[usize], block: usize) -> Run {
        run_cv(seq, params, ms, clocks, block, None)
    }

    /// As [`run_with`], with a Pattern CV patched when there is one.
    fn run_cv(seq: &mut ChordSequencer, params: &[f32], ms: usize, clocks: &[usize], block: usize, cv: Option<&[f32]>) -> Run {
        seq.prepare(1000.0, block);
        let mut run = Run {
            notes: vec![Vec::new(); MAX_VOICES],
            gates: vec![Vec::new(); MAX_VOICES],
            velocities: vec![Vec::new(); MAX_VOICES],
            root: Vec::new(),
            bass: Vec::new(),
            eoc: Vec::new(),
            channels: 0,
            mono_channels: 0,
        };
        let mut outputs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::polyphonic(block, SignalType::Control)).collect();
        let midi = |v: &f32| (v * 12.0 + 60.0).round();
        for start in (0..ms).step_by(block) {
            let mut clock = SignalBuffer::control(block);
            for &t in clocks.iter().filter(|&&t| (start..start + block).contains(&t)) {
                clock.samples[t - start] = 1.0;
            }
            let pattern = match cv {
                Some(cv) => {
                    let mut buf = SignalBuffer::control(block);
                    buf.samples.copy_from_slice(&cv[start..start + block]);
                    buf
                }
                None => SignalBuffer::unconnected(block, SignalType::Control),
            };
            let (reset, mut running) = (SignalBuffer::control(block), SignalBuffer::control(block));
            running.samples.fill(1.0);
            seq.process(&[&clock, &reset, &running, &pattern], &mut outputs, params, &ProcessContext::new(1000.0, block));
            run.channels = outputs[ChordSequencer::PORT_PITCH].channels();
            run.mono_channels = outputs[ChordSequencer::PORT_ROOT].channels().max(outputs[ChordSequencer::PORT_BASS].channels());
            for channel in 0..run.channels {
                run.notes[channel].extend(outputs[ChordSequencer::PORT_PITCH].voice(channel).samples.iter().map(midi));
                run.gates[channel].extend(&outputs[ChordSequencer::PORT_GATE].voice(channel).samples);
                run.velocities[channel].extend(&outputs[ChordSequencer::PORT_VELOCITY].voice(channel).samples);
            }
            run.root.extend(outputs[ChordSequencer::PORT_ROOT].samples.iter().map(midi));
            run.bass.extend(outputs[ChordSequencer::PORT_BASS].samples.iter().map(midi));
            run.eoc.extend(&outputs[ChordSequencer::PORT_EOC].samples);
        }
        run
    }

    fn run(params: &[f32], ms: usize, clocks: &[usize]) -> Run {
        run_with(&mut ChordSequencer::new(), params, ms, clocks, ms)
    }

    fn rises(gate: &[f32]) -> Vec<usize> {
        (0..gate.len()).filter(|&t| gate[t] > 0.5 && (t == 0 || gate[t - 1] < 0.5)).collect()
    }

    #[test]
    fn info_ports_and_parameters() {
        let seq = ChordSequencer::new();
        assert_eq!(seq.info().id, "seq.chord");
        assert_eq!(seq.info().category, ModuleCategory::Utility);
        assert!(seq.polyphonic());
        let ids: Vec<_> = seq.ports().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["clock", "reset", "run", "pattern", "pitch", "gate", "velocity", "root", "bass", "eoc"]);
        let params = seq.parameters();
        // Pattern A's steps, then the Chain, then patterns B to D
        assert_eq!(params.len(), 12 + 8 * MAX_STEPS + CHAIN_SLOTS + 3 * 8 * MAX_STEPS);
        assert_eq!(params.len(), ChordSequencer::PARAM_COUNT);
        assert_eq!(params[ChordSequencer::PARAM_CHAIN].name, "Chain 1");
        assert_eq!(params[ChordSequencer::PARAM_CHAIN].default, 1.0, "the chain starts as just A");
        assert_eq!(params[ChordSequencer::step_param(1, 2, ChordField::Root)].name, "Step B3 Root");
        assert_eq!(params[ChordSequencer::step_param(3, 15, ChordField::Strum)].id, "step_d16_strum");
        assert_eq!(params[ChordSequencer::step_param(2, 0, ChordField::Type)].default, 3.0, "every pattern starts on Cmaj7");
        assert_eq!(params[ChordSequencer::PARAM_BASS_VOICE].id, "bass_voice");
        assert_eq!(params[ChordSequencer::PARAM_ROOTLESS].id, "rootless");
        assert_eq!(params[ChordSequencer::PARAM_STRUM].id, "strum");
        assert_eq!(params[ChordSequencer::PARAM_STRUM_DIR].id, "strum_dir");
        assert_eq!(params[ChordSequencer::PARAM_STRUM_TAPER].id, "strum_taper");
        assert_eq!(params[ChordSequencer::step_root_param(0)].name, "Step 1 Root");
        assert_eq!(params[ChordSequencer::step_type_param(15)].name, "Step 16 Type");
        assert_eq!(params[ChordSequencer::step_bass_param(2)].id, "step_3_bass");
        assert_eq!(params[ChordSequencer::step_tie_param(15)].id, "step_16_tie");
        assert_eq!(params[ChordSequencer::step_top_param(4)].id, "step_5_top");
        assert_eq!(params[ChordSequencer::step_strum_param(2)].id, "step_3_strum");
        assert_eq!(params[ChordSequencer::PARAM_CHAIN - 1].id, "step_16_strum");
        assert_eq!(params.last().unwrap().id, "step_d16_strum");
        for pattern in 0..PATTERNS {
            let letter = ["", "b", "c", "d"][pattern];
            for step in 0..MAX_STEPS {
                for field in ChordField::ALL {
                    let param = &params[ChordSequencer::step_param(pattern, step, field)];
                    assert_eq!(param.name, ChordSequencer::step_param_name(pattern, step, field));
                    let in_a = params[ChordSequencer::step_param(0, step, field)].id;
                    assert_eq!(param.id, in_a.replacen("step_", &format!("step_{letter}"), 1));
                }
            }
        }
        assert_eq!(TYPE_LABELS.len(), CHORD_TYPES.len());
        for (label, kind) in TYPE_LABELS.iter().zip(&CHORD_TYPES) {
            assert_eq!(*label, kind.label);
        }
    }

    #[test]
    fn chords_are_named_as_a_lead_sheet_writes_them() {
        assert_eq!(spec(57, kind("m9"), None).name(), "Am9");
        assert_eq!(spec(50, MAJ, Some(6)).name(), "D/F#");
        assert_eq!(spec(57, MIN, Some(8)).name(), "Am/G#");
        assert_eq!(spec(57, MIN, Some(7)).name(), "Am/G");
        assert_eq!(spec(48, MAJ, Some(4)).name(), "C/E");
        assert_eq!(spec(48, DOM7, Some(10)).name(), "C7/Bb");
        assert_eq!(spec(58, MAJ, None).name(), "Bb");
        assert_eq!(spec(54, kind("m7b5"), None).name(), "F#m7b5");
        assert_eq!(spec(51, MAJ7, Some(7)).name(), "Ebmaj7/G");
        assert_eq!(spec(53, MAJ, Some(7)).name(), "F/G");
        assert_eq!(spec(56, MAJ, Some(0)).name(), "Ab/C");
        // A spelling that would need a double flat falls back to the plain one
        assert_eq!(spec(49, MAJ, Some(2)).name(), "Db/D");
    }

    #[test]
    fn slash_bass_sits_at_or_below_the_root() {
        assert_eq!(spec(50, MAJ, Some(6)).bass_note(), 42, "D/F#: F#2 under D3");
        assert_eq!(spec(45, MIN, Some(8)).bass_note(), 44, "Am/G#: G#2 under A2");
        assert_eq!(spec(48, MAJ, Some(0)).bass_note(), 48, "C/C is the root");
        assert_eq!(spec(48, MAJ, None).bass_note(), 48);
    }

    #[test]
    fn close_voicing_sits_in_the_range_octave() {
        let cmaj7 = voice_chord(&spec(36, MAJ7, None), &close(4, 3), None);
        assert_eq!(cmaj7.notes(), [48, 52, 55, 59], "C3 E3 G3 B3, whatever octave the root was written in");
        let a = voice_chord(&spec(57, MIN, None), &close(3, 4), None);
        assert_eq!(a.notes(), [69, 72, 76], "A4 C5 E5");
    }

    #[test]
    fn open_and_spread_voicings() {
        let open = VoicingSettings { voicing: Voicing::Open, ..close(4, 3) };
        // Drop-2: C E G B with G dropped an octave is G C E B
        assert_eq!(voice_chord(&spec(48, MAJ7, None), &open, None).notes(), [55, 60, 64, 71]);
        let spread = VoicingSettings { voicing: Voicing::Spread, ..close(4, 3) };
        // Every other voice up an octave: C G E B
        assert_eq!(voice_chord(&spec(48, MAJ7, None), &spread, None).notes(), [48, 55, 64, 71]);
    }

    #[test]
    fn few_voices_leave_out_the_fifth_and_many_double_up() {
        // Cmaj9 in four voices: C E B D, no G
        assert_eq!(voice_chord(&spec(48, MAJ9, None), &close(4, 3), None).notes(), [48, 50, 52, 59]);
        // In three: the shell, C E B
        assert_eq!(voice_chord(&spec(48, MAJ9, None), &close(3, 3), None).notes(), [48, 52, 59]);
        // A triad in five voices doubles the root and 3rd an octave up
        assert_eq!(voice_chord(&spec(48, MAJ, None), &close(5, 3), None).notes(), [48, 52, 55, 60, 64]);
        // C13 in four: C E A Bb
        assert_eq!(voice_chord(&spec(48, kind("13"), None), &close(4, 3), None).notes(), [48, 52, 57, 58]);
        // One voice is the root
        assert_eq!(voice_chord(&spec(50, M7, None), &close(1, 3), None).notes(), [50]);
    }

    #[test]
    fn rootless_voicings_drop_the_root_first() {
        let rootless = VoicingSettings { rootless: true, ..close(4, 3) };
        // Cmaj9 without its root: E G B D
        assert_eq!(voice_chord(&spec(48, MAJ9, None), &rootless, None).notes(), [52, 55, 59, 62]);
        // With room for every tone, the root stays
        let wide = VoicingSettings { voices: 5, ..rootless };
        let chord = voice_chord(&spec(48, MAJ9, None), &wide, None);
        assert!(chord.notes().iter().any(|n| n % 12 == 0));
    }

    #[test]
    fn rootless_knob_leaves_the_root_to_a_bass_elsewhere() {
        let mut params = params_with(&[(50, kind("madd9"))]);
        params[ChordSequencer::PARAM_VOICES] = 3.0;
        params[ChordSequencer::PARAM_RANGE] = 4.0;
        assert!(!ChordSequencer::settings(&params, 3).0.rootless);
        params[ChordSequencer::PARAM_ROOTLESS] = 1.0;
        let (settings, bass_voice) = ChordSequencer::settings(&params, 3);
        assert!(settings.rootless && !bass_voice);
        // Dm(add9) without its D: F A E, the 9th on top
        assert_eq!(voice_chord(&ChordSequencer::spec(&params, 0), &settings, None).notes(), [65, 69, 76]);
        // Bass Voice leaves the root out whatever the knob says
        params[ChordSequencer::PARAM_ROOTLESS] = 0.0;
        params[ChordSequencer::PARAM_BASS_VOICE] = 1.0;
        assert!(ChordSequencer::settings(&params, 4).0.rootless);
    }

    #[test]
    fn minor_add9_is_a_minor_triad_with_a_9th_and_no_7th() {
        let dm_add9 = spec(50, kind("madd9"), None);
        assert_eq!(dm_add9.name(), "Dmadd9");
        assert_eq!(voice_chord(&dm_add9, &close(4, 3), None).notes(), [50, 52, 53, 57]);
        // With three voices the 5th goes first
        assert_eq!(voice_chord(&dm_add9, &close(3, 3), None).notes(), [50, 52, 53]);
    }

    #[test]
    fn top_puts_its_tone_on_top_in_the_range_octave() {
        let topped = |top| ChordSpec { top: Some(top), ..spec(48, MAJ, None) };
        // C major, E on top: G C E, its lowest note in the Range octave
        assert_eq!(voice_chord(&topped(4), &close(3, 4), None).notes(), [67, 72, 76]);
        assert_eq!(voice_chord(&topped(7), &close(3, 4), None).notes(), [60, 64, 67]);
        assert_eq!(voice_chord(&topped(0), &close(3, 4), None).notes(), [64, 67, 72]);
        // Open and spread voicings too
        for voicing in [Voicing::Open, Voicing::Spread] {
            let settings = VoicingSettings { voicing, ..close(4, 3) };
            for pc in [0, 4, 7, 11] {
                let chord = voice_chord(&ChordSpec { top: Some(pc), ..spec(48, MAJ7, None) }, &settings, None);
                let top = chord.notes()[chord.len - 1];
                if top % 12 != pc {
                    // Only when no inversion of this voicing can
                    let any = (0..4).any(|start| stack(&[0, 4, 7, 11], start, 4, voicing).notes()[3] % 12 == pc);
                    assert!(!any, "{voicing:?}: {pc} on top of {:?}", chord.notes());
                }
            }
        }
    }

    #[test]
    fn top_narrows_voice_leading_to_its_inversions() {
        let settings = close(3, 4);
        // From C E G, voice leading keeps C on the bottom: B D G for G major
        let c = voice_chord(&spec(48, MAJ, None), &settings, None);
        assert_eq!(c.notes(), [60, 64, 67]);
        let led = voice_chord(&spec(43, MAJ, None), &settings, Some(&c));
        assert_eq!(led.notes(), [59, 62, 67]);
        // With D on top it takes G B D, the nearest that has it there
        let topped = voice_chord(&ChordSpec { top: Some(2), ..spec(43, MAJ, None) }, &settings, Some(&c));
        assert_eq!(topped.notes(), [55, 59, 62]);
    }

    #[test]
    fn a_top_the_chord_doesnt_keep_is_ignored() {
        let settings = close(3, 4);
        // Cmaj7 in three voices leaves its G out: G on top can't be had
        let plain = voice_chord(&spec(48, MAJ7, None), &settings, None);
        let topped = voice_chord(&ChordSpec { top: Some(7), ..spec(48, MAJ7, None) }, &settings, None);
        assert_eq!(plain, topped);
        // Nor a note outside the chord
        let led = voice_chord(&ChordSpec { top: Some(1), ..spec(53, MAJ, None) }, &settings, Some(&plain));
        assert_eq!(led, voice_chord(&spec(53, MAJ, None), &settings, Some(&plain)));
    }

    #[test]
    fn kept_tones_stack_up_from_the_root() {
        let rootless = VoicingSettings { rootless: true, ..close(3, 4) };
        assert_eq!(kept_tones(&spec(50, kind("madd9"), None), &rootless), [5, 9, 4]);
        assert_eq!(kept_tones(&spec(48, MAJ7, None), &close(3, 4)), [0, 4, 11]);
        assert_eq!(kept_tones(&spec(48, MAJ7, None), &close(4, 4)), [0, 4, 7, 11]);
    }

    #[test]
    fn a_step_reads_its_top() {
        let mut params = params_with(&[(48, MAJ)]);
        assert_eq!(ChordSequencer::spec(&params, 0).top, None);
        params[ChordSequencer::step_top_param(0)] = 5.0;
        assert_eq!(ChordSequencer::spec(&params, 0).top, Some(4));
    }

    #[test]
    fn every_chord_type_voices_with_each_tone_once() {
        for (index, kind) in CHORD_TYPES.iter().enumerate() {
            let chord = voice_chord(&spec(48, index, None), &close(kind.tones.len(), 3), None);
            let mut classes: Vec<i32> = chord.notes().iter().map(|n| n.rem_euclid(12)).collect();
            classes.sort_unstable();
            let mut expected: Vec<i32> = kind.tones.iter().map(|t| t.rem_euclid(12)).collect();
            expected.sort_unstable();
            assert_eq!(classes, expected, "{}", kind.label);
            assert!(chord.notes().windows(2).all(|w| w[0] < w[1]), "{}: {:?}", kind.label, chord.notes());
            assert!(chord.notes()[chord.len - 1] - chord.notes()[0] < 12, "{}: a close voicing fits an octave", kind.label);
        }
    }

    #[test]
    fn voice_leading_moves_a_two_five_one_by_two_semitones_at_most() {
        // ii-V-I in every key and range, close voicing: each voice moves by a
        // step or less, as the textbook voicing does
        for key in 0..12 {
            for range in 2..=5 {
                let mut params = params_with(&[(48 + (key + 2) % 12, M7), (48 + (key + 7) % 12, DOM7), (48 + key, MAJ7)]);
                params[ChordSequencer::PARAM_RANGE] = range as f32;
                let played = run(&params, 300, &[0, 100, 200]);
                let chords: Vec<_> = [50, 150, 250].iter().map(|&t| played.chord(t)).collect();
                for pair in chords.windows(2) {
                    for (from, to) in pair[0].iter().zip(&pair[1]) {
                        assert!((from - to).abs() <= 2, "key {key}, range {range}: {:?} -> {:?}", pair[0], pair[1]);
                    }
                }
            }
        }
    }

    #[test]
    fn the_manual_two_five_one() {
        // The voicings the manual prints, Range 3, four voices
        let params = params_with(&[(50, M7), (55, DOM7), (48, MAJ7)]);
        let played = run(&params, 300, &[0, 100, 200]);
        assert_eq!(played.chord(50), [50, 53, 57, 60], "D3 F3 A3 C4");
        assert_eq!(played.chord(150), [50, 53, 55, 59], "D3 F3 G3 B3");
        assert_eq!(played.chord(250), [48, 52, 55, 59], "C3 E3 G3 B3");
    }

    #[test]
    fn a_looping_pattern_plays_the_voicings_foreseen() {
        // ii-V-I, a rest, then IV, looped three times: by the third time
        // round, the chords sound as looped_voicings foresees them
        let mut params = params_with(&[(50, M7), (55, DOM7), (48, MAJ7), (57, M7), (53, MAJ7)]);
        params[ChordSequencer::step_gate_param(3)] = 0.0;
        let clocks: Vec<usize> = (0..15).map(|i| i * 100).collect();
        let played = run(&params, 1500, &clocks);
        let specs: Vec<ChordSpec> = (0..5).map(|s| ChordSequencer::spec(&params, s)).collect();
        let gates: Vec<bool> = (0..5).map(|s| params[ChordSequencer::step_gate_param(s)] > 0.5).collect();
        let (settings, _) = ChordSequencer::settings(&params, played.channels);
        let voiced = looped_voicings(&specs, &gates, &[0, 1, 2, 3, 4], &settings);
        for step in [0, 1, 2, 4] {
            assert_eq!(played.chord(1000 + step * 100 + 50), voiced[step].unwrap().notes(), "step {}", step + 1);
        }
        assert_eq!(voiced[3], None, "a rest isn't voiced");
        // ...which isn't how it began: the first time round, step 1 was in root position
        assert_ne!(played.chord(50), voiced[0].unwrap().notes());
    }

    #[test]
    fn a_reset_starts_the_voicings_afresh() {
        // Played twice round, with a reset between: the same chords both
        // times, though the last chord would otherwise lead the first
        let mut params = params_with(&[(50, M7), (55, DOM7), (48, MAJ7)]);
        params[ChordSequencer::PARAM_VOICES] = 4.0;
        let mut seq = ChordSequencer::new();
        seq.prepare(1000.0, 1);
        let mut outputs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::polyphonic(1, SignalType::Control)).collect();
        let mut chords = Vec::new();
        for t in 0..700 {
            let mut clock = SignalBuffer::control(1);
            let mut reset = SignalBuffer::control(1);
            clock.samples[0] = if [0, 100, 200, 400, 500, 600].contains(&t) { 1.0 } else { 0.0 };
            reset.samples[0] = if t == 350 { 1.0 } else { 0.0 };
            seq.process(&[&clock, &reset], &mut outputs, &params, &ProcessContext::new(1000.0, 1));
            if t % 100 == 50 {
                chords.push((0..4).map(|c| (outputs[0].voice(c).samples[0] * 12.0 + 60.0).round() as i32).collect::<Vec<_>>());
            }
        }
        assert_eq!(chords[..3], chords[4..7]);
    }

    #[test]
    fn rootless_maj9_over_a_bass() {
        // The manual: Voices 4 with a bass voice is C under E B D; Voices 5
        // is C under E G B D
        for (voices, chord) in [(4.0, vec![48, 52, 59, 62]), (5.0, vec![48, 52, 55, 59, 62])] {
            let mut params = params_with(&[(48, MAJ9)]);
            params[ChordSequencer::PARAM_BASS_VOICE] = 1.0;
            params[ChordSequencer::PARAM_VOICES] = voices;
            // The bass is C3, the root as written
            assert_eq!(run(&params, 10, &[0]).chord(5), chord, "{voices} voices");
        }
    }

    #[test]
    fn without_voice_leading_every_chord_is_in_root_position() {
        let mut params = params_with(&[(50, M7), (55, DOM7), (48, MAJ7)]);
        params[ChordSequencer::PARAM_VOICE_LEADING] = 0.0;
        let played = run(&params, 300, &[0, 100, 200]);
        assert_eq!(played.chord(50), [50, 53, 57, 60]);
        assert_eq!(played.chord(150), [55, 59, 62, 65]);
        assert_eq!(played.chord(250), [48, 52, 55, 59]);
    }

    #[test]
    fn a_long_progression_stays_near_its_range() {
        // A thousand random chords, voice-led: the lowest note never strays
        // more than a fifth from the Range octave
        let mut state = 7u32;
        let mut random = |n: usize| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as usize % n
        };
        let settings = close(4, 3);
        let mut previous: Option<Chord> = None;
        for _ in 0..1000 {
            let root = 40 + random(12) as i32;
            let chord = voice_chord(&spec(root, random(CHORD_TYPES.len()), None), &settings, previous.as_ref());
            assert!((48 - 5..48 + 12 + 5).contains(&chord.notes[0]), "{:?}", chord.notes());
            previous = Some(chord);
        }
    }

    #[test]
    fn cables_carry_one_channel_per_voice_and_root_and_bass_are_mono() {
        for voices in 1..=MAX_VOICES {
            let mut params = params_with(&[(48, MAJ)]);
            params[ChordSequencer::PARAM_VOICES] = voices as f32;
            let played = run(&params, 10, &[0]);
            assert_eq!(played.channels, voices);
            assert_eq!(played.mono_channels, 1);
            for channel in 0..voices {
                assert_eq!(played.gates[channel][5], 1.0, "{voices} voices: channel {channel} plays");
            }
        }
    }

    #[test]
    fn gates_retrigger_each_chord_and_a_tie_holds_it() {
        let mut params = params_with(&[(48, MAJ), (53, MAJ), (55, MAJ), (48, MAJ)]);
        // Step 2 ties into step 3
        params[ChordSequencer::step_tie_param(1)] = 1.0;
        let played = run(&params, 4000, &[0, 1000, 2000, 3000]);
        for channel in 0..4 {
            let gate = &played.gates[channel];
            // At 100% every chord holds to the next clock, then dips a
            // sample for the next; across the tie there's no dip at all
            assert_eq!(rises(gate), [0, 1001, 3001], "channel {channel}");
            assert!(gate[1001..3000].iter().all(|&g| g == 1.0), "channel {channel} holds across the tie");
        }
        // The chord changes under the held gate
        assert_eq!(played.root[1500], 53.0);
        assert_eq!(played.root[2500], 55.0);
        assert_ne!(played.chord(1500), played.chord(2500));
    }

    /// Params with a strum, `ms` between channels, in the direction given.
    fn strummed(chords: &[(i32, usize)], ms: f32, direction: StrumDirection) -> Vec<f32> {
        let mut params = params_with(chords);
        params[ChordSequencer::PARAM_STRUM] = ms;
        params[ChordSequencer::PARAM_STRUM_DIR] = direction as usize as f32;
        params
    }

    /// When each channel's gate rose, channel by channel.
    fn rises_by_channel(played: &Run) -> Vec<Vec<usize>> {
        (0..played.channels).map(|c| rises(&played.gates[c])).collect()
    }

    #[test]
    fn strum_staggers_each_channels_gate() {
        // 20 ms apart at 1 kHz, lowest channel first. The second chord
        // re-strikes each channel in turn: a sample's dip, then the rise
        let played = run(&strummed(&[(48, MAJ), (53, MAJ)], 20.0, StrumDirection::Up), 2000, &[0, 1000]);
        assert_eq!(rises_by_channel(&played), [[0, 1001], [20, 1021], [40, 1041], [60, 1061]]);
        let mut down = strummed(&[(48, MAJ), (53, MAJ)], 20.0, StrumDirection::Down);
        down[ChordSequencer::PARAM_VOICES] = 3.0;
        let played = run(&down, 2000, &[0, 1000]);
        assert_eq!(rises_by_channel(&played), [[40, 1041], [20, 1021], [0, 1001]]);
    }

    #[test]
    fn a_strummed_channel_rings_its_old_note_until_its_turn() {
        let played = run(&strummed(&[(48, MAJ), (53, MAJ)], 20.0, StrumDirection::Up), 2000, &[0, 1000]);
        let (c, f) = (played.chord(500), played.chord(1500));
        for channel in 0..4 {
            let dip = 1000 + 20 * channel;
            let notes = &played.notes[channel];
            let gate = &played.gates[channel];
            // The old chord, gate still high, up to the strike
            assert!((1000..dip).all(|t| notes[t] as i32 == c[channel] && gate[t] == 1.0), "channel {channel} rings C until {dip}");
            // The new pitch on the dip, before the gate rises on it
            assert_eq!((notes[dip] as i32, gate[dip]), (f[channel], 0.0), "channel {channel}");
            assert_eq!((notes[dip + 1] as i32, gate[dip + 1]), (f[channel], 1.0), "channel {channel}");
        }
    }

    #[test]
    fn alt_strums_down_and_up_in_time_with_the_clock() {
        // Up, down, up over a rest without striking, then down and up: the
        // hand never stops moving
        let mut params = strummed(&[(48, MAJ), (53, MAJ), (55, MAJ), (48, MAJ), (53, MAJ)], 10.0, StrumDirection::Alternate);
        params[ChordSequencer::PARAM_VOICES] = 3.0;
        params[ChordSequencer::PARAM_GATE_LENGTH] = 50.0;
        params[ChordSequencer::step_gate_param(2)] = 0.0;
        let played = run(&params, 5000, &[0, 1000, 2000, 3000, 4000]);
        assert_eq!(rises_by_channel(&played), [[0, 1020, 3020, 4000], [10, 1010, 3010, 4010], [20, 1000, 3000, 4020]]);
    }

    #[test]
    fn the_taper_softens_the_strings_struck_later() {
        // Four strings: the first at the step's velocity, the last at none
        let near = |turns: [f32; 4], expected: [f32; 4]| turns.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-6);
        assert!(near([0, 1, 2, 3].map(|turn| taper_scale(turn, 4, 1.0)), [1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0]));
        // Below 0 the first is softest and the last full
        assert!(near([0, 1, 2, 3].map(|turn| taper_scale(turn, 4, -0.5)), [0.5, 2.0 / 3.0, 5.0 / 6.0, 1.0]));
        assert_eq!(taper_scale(0, 1, 1.0), 1.0, "one string is struck as written");
        assert!((0..4).all(|turn| taper_scale(turn, 4, 0.0) == 1.0), "no taper, no change");
    }

    #[test]
    fn each_channels_velocity_follows_the_taper_in_strum_order() {
        // Velocity 127, Taper 60%: up from the lowest channel on the first
        // chord, down from the highest on the second
        let mut params = strummed(&[(48, MAJ), (53, MAJ)], 20.0, StrumDirection::Alternate);
        params[ChordSequencer::PARAM_STRUM_TAPER] = 60.0;
        params[ChordSequencer::step_velocity_param(0)] = 127.0;
        params[ChordSequencer::step_velocity_param(1)] = 63.5;
        let played = run(&params, 2000, &[0, 1000]);
        let velocity = |channel: usize, t: usize| played.velocities[channel][t];
        let up = [1.0, 0.8, 0.6, 0.4];
        for channel in 0..4 {
            assert!((velocity(channel, 500) - up[channel]).abs() < 1e-6, "channel {channel} up: {}", velocity(channel, 500));
            let down = 0.5 * up[3 - channel];
            assert!((velocity(channel, 1500) - down).abs() < 1e-6, "channel {channel} down: {}", velocity(channel, 1500));
        }
        // A channel the strum hasn't reached yet rings on as hard as it was
        // struck; it takes the new step's velocity with its new note
        assert_eq!(velocity(0, 1059), velocity(0, 500), "channel 1 is struck last, at 1060");
        assert_eq!(velocity(0, 1060), velocity(0, 1500));
    }

    #[test]
    fn a_steps_own_stroke_overrides_strum_dir_for_that_step_only() {
        // Strum Dir Up, but step 2 is written Down: steps 1 and 3 still
        // strum up
        let mut params = strummed(&[(48, MAJ), (53, MAJ), (55, MAJ)], 10.0, StrumDirection::Up);
        params[ChordSequencer::PARAM_VOICES] = 3.0;
        params[ChordSequencer::PARAM_GATE_LENGTH] = 50.0;
        params[ChordSequencer::step_strum_param(1)] = 2.0;
        let played = run(&params, 3000, &[0, 1000, 2000]);
        assert_eq!(rises_by_channel(&played), [[0, 1020, 2000], [10, 1010, 2010], [20, 1000, 2020]]);

        // Under Alt, a step's Up stands where a down was due, and the hand
        // keeps time: the stroke after it is up again, as the clock has it
        params[ChordSequencer::PARAM_STRUM_DIR] = StrumDirection::Alternate as usize as f32;
        params[ChordSequencer::step_strum_param(1)] = 1.0;
        let played = run(&params, 3000, &[0, 1000, 2000]);
        assert_eq!(rises_by_channel(&played), [[0, 1000, 2000], [10, 1010, 2010], [20, 1020, 2020]]);
    }

    #[test]
    fn a_strum_fits_inside_its_step() {
        // 100 ms per channel won't fit a 100 ms step of four channels: once
        // the clock is measured, each waits a quarter of the step at most.
        // Before it is, the first strum takes the whole 100 ms each, and the
        // next clock comes before channels 2 to 4 are struck
        let played = run(&strummed(&[(48, MAJ), (53, MAJ), (55, MAJ)], 100.0, StrumDirection::Up), 300, &[0, 100, 200]);
        assert_eq!(rises_by_channel(&played), [vec![0, 101, 201], vec![125, 226], vec![150, 251], vec![175, 276]]);
    }

    #[test]
    fn a_strum_never_restrikes_across_a_tie() {
        // Step 2 ties into step 3: step 3's notes still move in strum order,
        // under gates that never drop
        let mut params = strummed(&[(48, MAJ), (53, MAJ), (55, MAJ)], 20.0, StrumDirection::Up);
        params[ChordSequencer::step_tie_param(1)] = 1.0;
        let played = run(&params, 3000, &[0, 1000, 2000]);
        let (f, g) = (played.chord(1500), played.chord(2500));
        for channel in 0..4 {
            assert_eq!(rises(&played.gates[channel]), [20 * channel, 1001 + 20 * channel], "channel {channel}");
            let held = 1001 + 20 * channel..3000;
            assert!(played.gates[channel][held].iter().all(|&g| g == 1.0), "channel {channel} holds across the tie");
            let moves = 2000 + 20 * channel;
            assert_eq!(played.notes[channel][moves - 1] as i32, f[channel]);
            assert_eq!(played.notes[channel][moves] as i32, g[channel], "channel {channel} moves on its turn");
        }
    }

    #[test]
    fn a_strummed_chord_can_be_edited_while_it_plays() {
        // The channels the strum has reached follow an edit at once; the
        // rest take the edited chord when it reaches them
        let mut seq = ChordSequencer::new();
        let mut params = strummed(&[(48, MAJ)], 20.0, StrumDirection::Up);
        run_with(&mut seq, &params, 30, &[0], 30);
        params[ChordSequencer::step_type_param(0)] = MIN as f32;
        let after = run_with(&mut seq, &params, 70, &[], 70);
        assert_eq!(rises_by_channel(&after)[2..], [[10], [30]], "channels 3 and 4 struck 40 and 60 ms in");
        assert_eq!(after.chord(60), [48, 51, 55, 60], "C minor on every channel");
    }

    #[test]
    fn envelopes_are_not_retriggered_across_a_tie() {
        use crate::modules::envelope::AdsrEnvelope;

        // A rest, then F tied into G: a slow attack from F carries on
        // rising through G
        let mut params = params_with(&[(48, MAJ), (53, MAJ), (55, MAJ)]);
        params[ChordSequencer::step_gate_param(0)] = 0.0;
        params[ChordSequencer::step_tie_param(1)] = 1.0;
        params[ChordSequencer::PARAM_GATE_LENGTH] = 50.0;
        let played = run(&params, 3000, &[0, 1000, 2000]);
        for channel in 0..4 {
            let mut env = AdsrEnvelope::new();
            env.prepare(1000.0, 2000);
            let mut env_params: Vec<f32> = env.parameters().iter().map(|p| p.default).collect();
            for (i, p) in env.parameters().iter().enumerate() {
                match p.name {
                    "Attack" => env_params[i] = 1.4,
                    "Sustain" => env_params[i] = 1.0,
                    _ => {}
                }
            }
            let mut gate = SignalBuffer::control(2000);
            gate.samples.copy_from_slice(&played.gates[channel][1000..3000]);
            let outputs = env.ports().iter().filter(|p| p.is_output()).count();
            let mut out: Vec<SignalBuffer> = (0..outputs).map(|_| SignalBuffer::control(2000)).collect();
            env.process(&[&gate], &mut out, &env_params, &ProcessContext::new(1000.0, 2000));
            let level = &out[0].samples;
            assert!((1..1400).all(|t| level[t] >= level[t - 1]), "channel {channel} rises without a break across the tie");
            assert!(level[1399] > 0.95, "channel {channel} reaches the peak on time: {}", level[1399]);
        }
    }

    #[test]
    fn the_first_chord_holds_until_the_next_clock() {
        // Before two clocks there's no step to measure, but a held chord
        // still lasts until the next one, so the first is as long as the rest
        let played = run(&params_with(&[(48, MAJ), (53, MAJ)]), 3000, &[0, 1500]);
        assert_eq!(rises(&played.gates[0]), [0, 1501]);
        assert!(played.gates[0][..1500].iter().all(|&g| g == 1.0));
        // Not forever, if the clock never comes
        let alone = run(&params_with(&[(48, MAJ)]), 6000, &[0]);
        assert_eq!(alone.gates[0][3999], 1.0);
        assert_eq!(alone.gates[0][4000], 0.0);
    }

    #[test]
    fn before_the_first_clock_the_first_chord_waits_silent() {
        let mut params = params_with(&[(53, MAJ7), (48, MAJ)]);
        params[ChordSequencer::PARAM_BASS_VOICE] = 1.0;
        params[ChordSequencer::PARAM_VOICES] = 5.0;
        let played = run(&params, 100, &[]);
        assert_eq!(played.chord(50), [53, 57, 60, 64, 65], "F3 under A C E, with Fmaj7's root on top");
        assert!(played.gates[..5].iter().all(|g| g.iter().all(|&g| g == 0.0)));
        assert_eq!(played.root[50], 53.0);
    }

    #[test]
    fn voices_and_voicing_reshape_a_held_chord_at_once() {
        // Even over a rest, the chord it holds takes the new settings
        let mut params = params_with(&[(48, MAJ7), (53, MAJ)]);
        params[ChordSequencer::step_gate_param(1)] = 0.0;
        let mut seq = ChordSequencer::new();
        run_with(&mut seq, &params, 200, &[0, 100], 200);
        params[ChordSequencer::PARAM_VOICES] = 3.0;
        let after = run_with(&mut seq, &params, 10, &[], 10);
        assert_eq!(after.channels, 3);
        assert_eq!(after.chord(5), [48, 52, 59], "Cmaj7's shell, C E B");
    }

    #[test]
    fn a_rest_keeps_the_last_chord_for_its_release() {
        let mut params = params_with(&[(48, MAJ), (53, MIN)]);
        params[ChordSequencer::step_gate_param(1)] = 0.0;
        let played = run(&params, 2000, &[0, 1000]);
        assert_eq!(played.chord(1500), played.chord(500));
        assert_eq!(played.gates[0][1500], 0.0);
        assert_eq!(played.root[1500], 48.0);
    }

    #[test]
    fn root_and_bass_follow_slash_chords() {
        // The line cliche: Am, Am/G#, Am/G, D/F#
        let mut params = params_with(&[(45, MIN), (45, MIN), (45, MIN), (50, MAJ)]);
        for (step, bass) in [(1, 8), (2, 7), (3, 6)] {
            params[ChordSequencer::step_bass_param(step)] = (bass + 1) as f32;
        }
        let played = run(&params, 400, &[0, 100, 200, 300]);
        let at = |t: usize| (played.root[t], played.bass[t]);
        assert_eq!(at(50), (45.0, 45.0));
        assert_eq!(at(150), (45.0, 44.0));
        assert_eq!(at(250), (45.0, 43.0));
        assert_eq!(at(350), (50.0, 42.0));
    }

    #[test]
    fn bass_voice_puts_the_bass_on_the_first_channel() {
        let mut params = params_with(&[(45, MIN), (50, MAJ)]);
        params[ChordSequencer::PARAM_BASS_VOICE] = 1.0;
        params[ChordSequencer::PARAM_VOICES] = 4.0;
        params[ChordSequencer::step_bass_param(1)] = 7.0; // D/F#
        let played = run(&params, 200, &[0, 100]);
        assert_eq!(played.channels, 4);
        assert_eq!(played.chord(50)[0], 45, "A2 under Am");
        assert_eq!(played.chord(150)[0], 42, "F#2 under D");
        // The three above it are the chord, A C E
        let mut above: Vec<i32> = played.chord(50)[1..].iter().map(|n| n % 12).collect();
        above.sort_unstable();
        assert_eq!(above, [0, 4, 9]);
    }

    #[test]
    fn an_edit_to_the_chord_playing_is_heard_at_once() {
        let mut seq = ChordSequencer::new();
        let mut params = params_with(&[(48, MAJ)]);
        let first = run_with(&mut seq, &params, 100, &[0], 100);
        assert_eq!(first.chord(50), [48, 52, 55, 60]);
        params[ChordSequencer::step_type_param(0)] = MIN as f32;
        let mut outputs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::polyphonic(10, SignalType::Control)).collect();
        seq.process(&[&SignalBuffer::control(10)], &mut outputs, &params, &ProcessContext::new(1000.0, 10));
        let notes: Vec<i32> = (0..4).map(|c| (outputs[0].voice(c).samples[5] * 12.0 + 60.0).round() as i32).collect();
        assert_eq!(notes, [48, 51, 55, 60]);
    }

    #[test]
    fn eoc_and_readout() {
        let params = params_with(&[(48, MAJ), (53, MAJ)]);
        let mut seq = ChordSequencer::new();
        let played = run_with(&mut seq, &params, 300, &[0, 100, 200], 300);
        assert_eq!(rises(&played.eoc), [200], "the third clock wraps round");
        let readout = seq.readout(&params).unwrap();
        assert_eq!(readout.values[ChordSequencer::READOUT_STEP], 0.0);
        assert_eq!(&readout.values[1..5], &[48.0, 52.0, 55.0, 60.0]);
        assert_eq!(readout.values[5], -1.0);
    }

    #[test]
    fn the_default_pattern_is_a_progression() {
        let params: Vec<f32> = ChordSequencer::new().parameters().iter().map(|p| p.default).collect();
        let names: Vec<String> = (0..4).map(|s| ChordSequencer::spec(&params, s).name()).collect();
        assert_eq!(names, ["Cmaj7", "Am7", "Fmaj7", "G7"]);
    }

    #[test]
    fn processing_is_the_same_in_any_block_size() {
        let params = strummed(&[(50, M7), (55, DOM7), (48, MAJ7), (45, kind("m9"))], 15.0, StrumDirection::Alternate);
        let clocks: Vec<usize> = (0..12).map(|i| i * 97).collect();
        let whole = run_with(&mut ChordSequencer::new(), &params, 1200, &clocks, 1200);
        let small = run_with(&mut ChordSequencer::new(), &params, 1200, &clocks, 16);
        assert_eq!(whole.notes[..4], small.notes[..4]);
        assert_eq!(whole.gates[..4], small.gates[..4]);
    }

    /// A chord as the popover auditions it.
    fn audition(notes: &[u8], root: u8, bass: u8) -> Audition {
        let mut chord = Audition { len: notes.len() as u8, root, bass, velocity: 0.5, ..Audition::default() };
        chord.notes[..notes.len()].copy_from_slice(notes);
        chord
    }

    #[test]
    fn an_audition_sounds_without_a_clock() {
        let params = params_with(&[(48, MAJ7)]);
        let mut seq = ChordSequencer::new();
        let quiet = run_with(&mut seq, &params, 50, &[], 50);
        assert!(quiet.gates[..4].iter().flatten().all(|&g| g == 0.0), "nothing plays before a clock");

        // Am/G#, held
        seq.audition(Some(audition(&[56, 57, 60, 64], 57, 56)));
        let held = run_with(&mut seq, &params, 100, &[], 100);
        assert_eq!(held.chord(50), [56, 57, 60, 64]);
        assert_eq!((held.root[50], held.bass[50]), (57.0, 56.0));
        for gate in &held.gates[..4] {
            assert_eq!(rises(gate), [1], "every gate rises together, after one low sample");
            assert!(gate[1..].iter().all(|&g| g == 1.0), "and stays up while the hand is down");
        }
        let readout = seq.readout(&params).unwrap();
        assert_eq!(&readout.values[1..6], &[56.0, 57.0, 60.0, 64.0, -1.0], "the face shows it");

        // Let go: the gates fall, and the notes ring where they were
        seq.audition(None);
        let after = run_with(&mut seq, &params, 100, &[], 100);
        assert!(after.gates[..4].iter().flatten().all(|&g| g == 0.0));
        assert_eq!(after.chord(99), [56, 57, 60, 64], "the release fades on the chord heard");
    }

    #[test]
    fn an_audition_strikes_over_a_chord_already_sounding() {
        let params = params_with(&[(48, MAJ7)]);
        let mut seq = ChordSequencer::new();
        run_with(&mut seq, &params, 50, &[0], 50);
        seq.audition(Some(audition(&[45, 48, 52, 55], 45, 45)));
        let struck = run_with(&mut seq, &params, 10, &[], 10);
        for gate in &struck.gates[..4] {
            assert_eq!(gate[..3], [0.0, 1.0, 1.0], "an open envelope starts again");
        }
        // A note more under the same hand moves the pitches, without a new attack
        seq.audition(Some(audition(&[43, 45, 48, 52], 45, 43)));
        let moved = run_with(&mut seq, &params, 10, &[], 10);
        assert_eq!(moved.chord(0), [43, 45, 48, 52]);
        assert!(moved.gates[..4].iter().flatten().all(|&g| g == 1.0));
    }

    #[test]
    fn a_chord_chosen_over_one_ringing_strikes_again() {
        // The popover's chips: Am7 chosen, then Am9 while it still rings. A
        // let-go and a new chord before the next block are a new attack
        let params = params_with(&[(48, MAJ7)]);
        let mut seq = ChordSequencer::new();
        seq.audition(Some(audition(&[45, 48, 52, 55], 45, 45)));
        run_with(&mut seq, &params, 50, &[], 50);
        seq.audition(None);
        seq.audition(Some(audition(&[45, 48, 52, 59], 45, 45)));
        let again = run_with(&mut seq, &params, 10, &[], 10);
        for gate in &again.gates[..4] {
            assert_eq!(gate[..3], [0.0, 1.0, 1.0]);
        }
        assert_eq!(again.chord(5), [45, 48, 52, 59]);
    }

    #[test]
    fn a_key_on_the_popover_piano_sounds_alone() {
        let params = params_with(&[(48, MAJ7)]);
        let mut seq = ChordSequencer::new();
        seq.audition(Some(audition(&[50], 50, 50)));
        let key = run_with(&mut seq, &params, 10, &[], 10);
        assert_eq!(rises(&key.gates[0]), [1]);
        assert!(key.gates[1..4].iter().flatten().all(|&g| g == 0.0), "only the first string");
        assert_eq!(key.root[5], 50.0);
    }

    #[test]
    fn an_audition_is_strummed_as_its_step_will_be() {
        // 20 ms between strings, Taper 60%: up from the lowest string, each
        // gate rising the sample after the strum reaches it
        let mut params = strummed(&[(48, MAJ7)], 20.0, StrumDirection::Up);
        params[ChordSequencer::PARAM_STRUM_TAPER] = 60.0;
        let mut seq = ChordSequencer::new();
        let am7 = audition(&[45, 48, 52, 55], 45, 45);
        seq.audition(Some(am7));
        let up = run_with(&mut seq, &params, 200, &[], 200);
        assert_eq!(rises_by_channel(&up), [[1], [21], [41], [61]]);
        for channel in 0..4 {
            let expected = 0.5 * taper_scale(channel, 4, 0.6);
            assert!((up.velocities[channel][100] - expected).abs() < 1e-6, "channel {channel}: {}", up.velocities[channel][100]);
        }

        // Down from the highest, with the taper following the stroke
        seq.audition(None);
        run_with(&mut seq, &params, 100, &[], 100);
        seq.audition(Some(Audition { down: true, ..am7 }));
        let down = run_with(&mut seq, &params, 200, &[], 200);
        assert_eq!(rises_by_channel(&down), [[61], [41], [21], [1]]);
        assert!(down.velocities[3][100] > down.velocities[0][100], "the top string is struck hardest");

        // A finger landing mid-strum moves the pitches without striking
        // again: the strings not yet reached take the new notes on their turn
        seq.audition(None);
        run_with(&mut seq, &params, 100, &[], 100);
        seq.audition(Some(am7));
        run_with(&mut seq, &params, 30, &[], 30);
        seq.audition(Some(audition(&[43, 48, 52, 57], 45, 43)));
        let landed = run_with(&mut seq, &params, 100, &[], 100);
        assert!(landed.gates[..2].iter().flatten().all(|&g| g == 1.0), "the strings struck ring on");
        assert_eq!(rises(&landed.gates[2]), [11]);
        assert_eq!(rises(&landed.gates[3]), [31]);
        assert_eq!(landed.chord(99), [43, 48, 52, 57]);
    }

    #[test]
    fn a_strummed_audition_rings_the_sequence_until_its_turn() {
        let params = strummed(&[(48, MAJ7)], 20.0, StrumDirection::Up);
        let mut seq = ChordSequencer::new();
        let before = run_with(&mut seq, &params, 100, &[0], 100);
        seq.audition(Some(audition(&[45, 48, 52, 55], 45, 45)));
        let struck = run_with(&mut seq, &params, 100, &[], 100);
        for channel in 0..4 {
            let strike = 20 * channel;
            let (notes, gate) = (&struck.notes[channel], &struck.gates[channel]);
            // Cmaj7 rings on, gate up, until the strum reaches the channel
            assert!((0..strike).all(|t| notes[t] == before.notes[channel][99] && gate[t] == 1.0), "channel {channel}");
            // Then a sample's dip on the new note, and the rise
            assert_eq!((notes[strike] as i32, gate[strike]), ([45, 48, 52, 55][channel], 0.0), "channel {channel}");
            assert_eq!(gate[strike + 1], 1.0);
        }
    }

    #[test]
    fn the_sequence_comes_back_on_its_next_chord() {
        let params = params_with(&[(48, MAJ7), (45, M7)]);
        let mut seq = ChordSequencer::new();
        run_with(&mut seq, &params, 100, &[0], 100);
        // Held over a clock: the audition goes on sounding
        seq.audition(Some(audition(&[50, 53, 57, 60], 50, 50)));
        let held = run_with(&mut seq, &params, 100, &[50], 100);
        assert_eq!(held.chord(99), [50, 53, 57, 60]);
        assert!(held.gates[..4].iter().all(|gate| gate[99] == 1.0));
        // Let go: the chord the clock struck underneath isn't heard
        seq.audition(None);
        let after = run_with(&mut seq, &params, 100, &[50], 100);
        assert!(after.gates[..4].iter().all(|gate| gate[..50].iter().all(|&g| g == 0.0)));
        assert_eq!(after.chord(49), [50, 53, 57, 60], "the audition rings until the next chord");
        // The next clock plays the sequence's own chord, Cmaj7
        assert!(after.gates[..4].iter().all(|gate| rises(gate) == [50]));
        assert_eq!(after.chord(60), [48, 52, 55, 59]);
        assert_eq!(after.root[60], 48.0);
    }

    #[test]
    fn stopping_forgets_an_audition() {
        let params = params_with(&[(48, MAJ7)]);
        let mut seq = ChordSequencer::new();
        seq.audition(Some(audition(&[60, 64, 67], 60, 60)));
        seq.reset();
        let after = run_with(&mut seq, &params, 20, &[], 20);
        assert!(after.gates[..4].iter().flatten().all(|&g| g == 0.0));
        assert_eq!(after.chord(0), [48, 52, 55, 59]);
    }

    #[test]
    fn is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ChordSequencer>();
    }

    /// Writes a pattern's chords, as `params_with` writes A's.
    fn fill(params: &mut [f32], pattern: usize, chords: &[(i32, usize)]) {
        for (step, &(root, kind)) in chords.iter().enumerate() {
            params[ChordSequencer::step_param(pattern, step, ChordField::Root)] = root as f32;
            params[ChordSequencer::step_param(pattern, step, ChordField::Type)] = kind as f32;
        }
    }

    fn set_chain(params: &mut [f32], patterns: &str) {
        for slot in 0..CHAIN_SLOTS {
            params[ChordSequencer::PARAM_CHAIN + slot] = 0.0;
        }
        for (slot, letter) in patterns.chars().enumerate() {
            params[ChordSequencer::PARAM_CHAIN + slot] = ("ABCD".find(letter).unwrap() + 1) as f32;
        }
    }

    /// The root at each clock.
    fn roots_at(run: &Run, clocks: &[usize]) -> Vec<i32> {
        clocks.iter().map(|&t| run.root[t] as i32).collect()
    }

    /// A clock every 100 ms.
    fn every_100(count: usize) -> Vec<usize> {
        (0..count).map(|n| n * 100).collect()
    }

    #[test]
    fn a_chain_plays_its_patterns_in_order() {
        // A verse in A, a chorus in B, the chain A A B
        let mut params = params_with(&[(36, MAJ), (41, MAJ)]);
        fill(&mut params, 1, &[(43, MAJ), (45, MIN)]);
        set_chain(&mut params, "AAB");
        let clocks = every_100(12);
        let played = run(&params, 1200, &clocks);
        assert_eq!(roots_at(&played, &clocks), [36, 41, 36, 41, 43, 45, 36, 41, 36, 41, 43, 45]);
        // EOC once the whole chain has played, on the clock that starts it again
        assert_eq!(rises(&played.eoc), [600]);
    }

    #[test]
    fn a_patch_from_before_patterns_plays_only_a() {
        // Patterns B to D aren't in an old patch, so they load as their
        // defaults, and the Chain is just A
        let params = params_with(&[(36, MAJ), (41, MAJ), (43, MAJ)]);
        let clocks = every_100(6);
        let played = run(&params, 600, &clocks);
        assert_eq!(roots_at(&played, &clocks), [36, 41, 43, 36, 41, 43]);
        assert_eq!(rises(&played.eoc), [300]);
    }

    #[test]
    fn pattern_cv_picks_the_pattern_at_each_pass() {
        let mut params = params_with(&[(36, MAJ), (41, MAJ)]);
        fill(&mut params, 1, &[(43, MAJ), (45, MIN)]);
        fill(&mut params, 2, &[(38, MIN), (31, MAJ)]);
        set_chain(&mut params, "AB");
        // The CV moves to C (0.6) halfway through the first pass, and the
        // pattern only changes where the next pass starts
        let mut cv = vec![0.1; 600];
        cv[150..].fill(0.6);
        let clocks = every_100(6);
        let played = run_cv(&mut ChordSequencer::new(), &params, 600, &clocks, 600, Some(&cv));
        assert_eq!(roots_at(&played, &clocks), [36, 41, 38, 31, 38, 31]);
        // Every pass ends the sequence, with no chain to play through
        assert_eq!(rises(&played.eoc), [200, 400]);
    }

    #[test]
    fn a_pattern_is_struck_as_hard_as_its_own_steps() {
        let mut params = params_with(&[(36, MAJ)]);
        fill(&mut params, 1, &[(43, MAJ)]);
        params[ChordSequencer::step_param(1, 0, ChordField::Velocity)] = 50.0;
        set_chain(&mut params, "AB");
        let played = run(&params, 200, &every_100(2));
        assert!((played.velocities[0][50] - 100.0 / 127.0).abs() < 1e-6);
        assert!((played.velocities[0][150] - 50.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn a_tie_carries_into_the_next_patterns_first_chord() {
        let mut params = params_with(&[(36, MAJ), (41, MAJ)]);
        fill(&mut params, 1, &[(43, MAJ), (45, MIN)]);
        params[ChordSequencer::step_tie_param(1)] = 1.0;
        set_chain(&mut params, "AB");
        let played = run(&params, 400, &every_100(4));
        // A's last chord ties into B's first: its notes move without a new attack
        assert_eq!(rises(&played.gates[0]), [0, 101, 301]);
        assert_eq!(played.root[250], 43.0);
    }

    #[test]
    fn reset_starts_the_chain_over() {
        let mut params = params_with(&[(36, MAJ), (41, MAJ)]);
        fill(&mut params, 1, &[(43, MAJ), (45, MIN)]);
        set_chain(&mut params, "AB");
        let mut seq = ChordSequencer::new();
        run_with(&mut seq, &params, 300, &every_100(3), 300);
        assert_eq!((seq.pattern, seq.current_step()), (1, 0));
        // A reset, then the next clock plays A's first chord
        let mut reset = SignalBuffer::control(1);
        reset.samples[0] = 1.0;
        let mut outputs: Vec<SignalBuffer> = (0..6).map(|_| SignalBuffer::polyphonic(1, SignalType::Control)).collect();
        seq.process(&[&SignalBuffer::control(1), &reset], &mut outputs, &params, &ProcessContext::new(1000.0, 1));
        let played = run_with(&mut seq, &params, 100, &[50], 100);
        assert_eq!(played.root[60], 36.0);
        assert_eq!(seq.pattern, 0);
    }

    #[test]
    fn the_readout_says_where_the_chain_is() {
        let mut params = params_with(&[(36, MAJ), (41, MAJ)]);
        set_chain(&mut params, "AB");
        let mut seq = ChordSequencer::new();
        let position = |seq: &ChordSequencer, params: &[f32]| PatternPosition::read(&seq.readout(params).unwrap(), ChordSequencer::READOUT_PATTERN);
        run_with(&mut seq, &params, 1, &[], 1);
        assert!(!position(&seq, &params).started);
        run_with(&mut seq, &params, 300, &[0, 100, 200], 300);
        let now = position(&seq, &params);
        assert_eq!((now.pattern, now.chain_slot, now.next_pattern, now.started, now.pattern_cv), (1, 1, 0, true, false));
        // Eight voices fill the notes without reaching the position
        params[ChordSequencer::PARAM_VOICES] = 8.0;
        run_with(&mut seq, &params, 100, &[0], 100);
        let readout = seq.readout(&params).unwrap();
        assert!(readout.values[ChordSequencer::READOUT_NOTES..ChordSequencer::READOUT_PATTERN].iter().all(|&n| n >= 0.0));
        assert_eq!(PatternPosition::read(&readout, ChordSequencer::READOUT_PATTERN).pattern, 1);
    }
}
