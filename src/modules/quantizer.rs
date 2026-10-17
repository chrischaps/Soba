//! Quantizer module.
//!
//! Snaps a V/Oct pitch to the nearest note of a scale, so random voltages,
//! LFOs and Sample & Hold play in key. A Trig pulse marks each new note,
//! ready to strike an envelope.

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    gate::Pulse,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    ParameterDisplay, SignalType,
};

/// Note names for the twelve pitch classes, from C.
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The Scale choices, in menu order. Custom is last.
pub const SCALE_NAMES: [&str; 11] = [
    "Chromatic",
    "Major",
    "Natural Minor",
    "Dorian",
    "Mixolydian",
    "Harmonic Minor",
    "Pentatonic Major",
    "Pentatonic Minor",
    "Blues",
    "Whole Tone",
    "Custom",
];

/// Index of Custom in [`SCALE_NAMES`]: the scale drawn on the mini piano.
pub const CUSTOM_SCALE: usize = SCALE_NAMES.len() - 1;

/// Each scale's notes as a 12-bit mask, bit `n` set when the note `n`
/// semitones above the root is in the scale.
const SCALE_MASKS: [u16; CUSTOM_SCALE] = [
    mask(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]), // Chromatic
    mask(&[0, 2, 4, 5, 7, 9, 11]),                 // Major
    mask(&[0, 2, 3, 5, 7, 8, 10]),                 // Natural Minor
    mask(&[0, 2, 3, 5, 7, 9, 10]),                 // Dorian
    mask(&[0, 2, 4, 5, 7, 9, 10]),                 // Mixolydian
    mask(&[0, 2, 3, 5, 7, 8, 11]),                 // Harmonic Minor
    mask(&[0, 2, 4, 7, 9]),                        // Pentatonic Major
    mask(&[0, 3, 5, 7, 10]),                       // Pentatonic Minor
    mask(&[0, 3, 5, 6, 7, 10]),                    // Blues
    mask(&[0, 2, 4, 6, 8, 10]),                    // Whole Tone
];

/// Every pitch class: the mask with all twelve bits set.
pub const ALL_NOTES: u16 = 0xFFF;

/// A mask with a bit set for each semitone listed.
const fn mask(semitones: &[u8]) -> u16 {
    let mut bits = 0;
    let mut i = 0;
    while i < semitones.len() {
        bits |= 1 << semitones[i];
        i += 1;
    }
    bits
}

/// A scale's notes relative to its root. Custom reads the saved mask.
pub fn scale_mask(scale: usize, custom: u16) -> u16 {
    SCALE_MASKS.get(scale).copied().unwrap_or(custom & ALL_NOTES)
}

/// Turns a mask relative to a root into absolute pitch classes, bit 0 = C.
pub fn rotate_to_key(relative: u16, root: i32) -> u16 {
    let shift = root.rem_euclid(12) as u32;
    let relative = relative & ALL_NOTES;
    ((relative << shift) | (relative >> (12 - shift))) & ALL_NOTES
}

/// Whether `note` (in semitones from C4) belongs to an absolute mask.
#[inline]
fn in_key(key: u16, note: i32) -> bool {
    key & (1 << note.rem_euclid(12)) != 0
}

/// The note of `key` closest to `semitones` (from C4). `key` must not be
/// empty. Any non-empty key has a note within six semitones of every pitch.
fn nearest_in_key(semitones: f32, key: u16) -> i32 {
    let center = semitones.round() as i32;
    let mut best = center;
    let mut best_distance = f32::INFINITY;
    for note in center - 7..=center + 7 {
        let distance = (semitones - note as f32).abs();
        if in_key(key, note) && distance < best_distance {
            best = note;
            best_distance = distance;
        }
    }
    best
}

/// Snaps a V/Oct pitch to a musical scale.
///
/// # Ports
///
/// **Inputs:**
/// - **In** (Control): V/Oct pitch to quantize. 0 V is C4, as on MIDI Note.
/// - **Transpose** (Control): Added to the Transpose knob in V/Oct, so a
///   sequencer's pitch moves the key by its own interval.
///
/// **Outputs:**
/// - **Out** (Control): The nearest scale note, plus Transpose, in V/Oct.
/// - **Trig** (Gate): A short pulse each time Out moves to a new note.
///
/// # Parameters
///
/// - **Root** (C-B): The scale's first note.
/// - **Scale**: Which notes are allowed. Custom uses the notes clicked on
///   the node's piano.
/// - **Transpose** (±12 st): Shifts Out, and so the key, by semitones.
/// - **Mask**: The Custom scale's notes as 12 bits, bit 0 the root.
///   Hidden; the piano edits it.
pub struct Quantizer {
    /// The scale note last chosen, in semitones from C4, before Transpose.
    /// Held through small wobbles at a boundary.
    note: Option<i32>,
    /// The note Out last carried, Transpose included, for Trig.
    out_note: Option<i32>,
    /// The Trig pulse.
    trig: Pulse,
    /// A new note arrived mid-pulse: drop Trig for a sample so the next
    /// pulse has an edge of its own.
    trig_gap: bool,
    /// Sample rate from the last prepare() call.
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl Quantizer {
    /// Creates a Quantizer set to C major.
    pub fn new() -> Self {
        let mut quantizer = Self {
            note: None,
            out_note: None,
            trig: Pulse::IDLE,
            trig_gap: false,
            sample_rate: 44100.0,
            ports: vec![
                PortDefinition::input_with_default("in", "In", SignalType::Control, 0.0).describe("V/Oct pitch to snap to the scale, such as a random voltage or an LFO"),
                PortDefinition::input_with_default("transpose_cv", "Transpose", SignalType::Control, 0.0).describe("V/Oct added to the Transpose knob: a sequencer's pitch here changes key"),
                PortDefinition::output("out", "Out", SignalType::Control).describe("The nearest note of the scale, transposed, in V/Oct"),
                PortDefinition::output("trig", "Trig", SignalType::Gate).describe("A short pulse each time Out moves to a new note, to strike an envelope"),
            ],
            parameters: vec![
                ParameterDefinition::choice("root", "Root", &NOTE_NAMES, 0).describe("The scale's first note"),
                ParameterDefinition::choice("scale", "Scale", &SCALE_NAMES, 1)
                    .describe("Which notes Out may play; Custom plays the keys lit on the piano"),
                ParameterDefinition::new("transpose", "Transpose", -12.0, 12.0, 0.0, ParameterDisplay::stepped("st"))
                    .describe("Shifts Out, and so the key, in semitones"),
                ParameterDefinition::new("mask", "Mask", 0.0, ALL_NOTES as f32, SCALE_MASKS[1] as f32, ParameterDisplay::stepped(""))
                    .describe("The Custom scale's notes, one bit per semitone above the root"),
            ],
        };
        quantizer.prepare(44100.0, 0);
        quantizer
    }

    const PORT_IN: usize = 0;
    const PORT_TRANSPOSE_CV: usize = 1;

    const PARAM_ROOT: usize = 0;
    const PARAM_SCALE: usize = 1;
    const PARAM_TRANSPOSE: usize = 2;
    const PARAM_MASK: usize = 3;

    /// How far, in semitones, the input must pass the midpoint between two
    /// notes before Out leaves the one it's on. A slow LFO wobbling at a
    /// boundary then holds its note instead of chattering between two.
    pub const HYSTERESIS: f32 = 0.2;

    /// Length of a Trig pulse.
    const TRIG_SECONDS: f32 = 0.01;

    /// The note of `key` for `semitones`, keeping the current note until
    /// the input is clearly closer to another.
    fn choose_note(&mut self, semitones: f32, key: u16) -> i32 {
        let nearest = nearest_in_key(semitones, key);
        if let Some(held) = self.note {
            // Past the midpoint by d, the new note is 2d closer than the old
            let closer_by = (semitones - held as f32).abs() - (semitones - nearest as f32).abs();
            if held != nearest && in_key(key, held) && closer_by < 2.0 * Self::HYSTERESIS {
                return held;
            }
        }
        self.note = Some(nearest);
        nearest
    }

    /// The next Trig sample.
    #[inline]
    fn next_trig(&mut self) -> f32 {
        if self.trig_gap {
            self.trig_gap = false;
            return 0.0;
        }
        self.trig.sample()
    }

    /// Starts a Trig pulse, with a one-sample gap first if one is running.
    fn fire_trig(&mut self) {
        self.trig_gap = self.trig.is_high();
        self.trig.fire(Self::TRIG_SECONDS, self.sample_rate);
    }
}

impl Default for Quantizer {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Quantizer {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: "util.quantizer",
            name: "Quantizer",
            category: ModuleCategory::Utility,
            description: "Snap a pitch to the nearest note of a scale",
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
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [out, trig_out, ..] = outputs else {
            return;
        };
        let root = params[Self::PARAM_ROOT].round() as i32;
        let scale = params[Self::PARAM_SCALE].round().max(0.0) as usize;
        let transpose_knob = params[Self::PARAM_TRANSPOSE];
        let custom = params[Self::PARAM_MASK].round().clamp(0.0, ALL_NOTES as f32) as u16;
        let key = rotate_to_key(scale_mask(scale, custom), root);

        let pitch_in = inputs.get(Self::PORT_IN);
        let transpose_cv = connected_input(inputs, Self::PORT_TRANSPOSE_CV);
        let at = |buffer: Option<&&SignalBuffer>, i: usize| buffer.map_or(0.0, |buf| buf.at(i));

        for i in 0..context.block_size {
            let semitones = at(pitch_in, i) * 12.0;
            let transpose = (transpose_knob + at(transpose_cv.as_ref(), i) * 12.0).round() as i32;

            if key == 0 {
                // No notes allowed: pass the pitch through untouched
                self.note = None;
                self.out_note = None;
                out.samples[i] = (semitones + transpose as f32) / 12.0;
            } else {
                let note = self.choose_note(semitones, key) + transpose;
                if self.out_note != Some(note) {
                    self.out_note = Some(note);
                    self.fire_trig();
                }
                out.samples[i] = note as f32 / 12.0;
            }
            trig_out.samples[i] = self.next_trig();
        }
    }

    fn reset(&mut self) {
        self.note = None;
        self.out_note = None;
        self.trig.clear();
        self.trig_gap = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Poly;

    const SAMPLE_RATE: f32 = 48000.0;
    const BLOCK: usize = 256;

    /// Root, Scale, Transpose, Mask, as the knobs would set them.
    fn params(root: usize, scale: usize, transpose: f32) -> [f32; 4] {
        [root as f32, scale as f32, transpose, SCALE_MASKS[1] as f32]
    }

    /// Runs `pitch` (V/Oct, one value per sample) through a Quantizer,
    /// returning (Out, Trig).
    fn run_with(q: &mut Quantizer, pitch: &[f32], transpose_cv: Option<&[f32]>, params: &[f32]) -> (Vec<f32>, Vec<f32>) {
        q.prepare(SAMPLE_RATE, BLOCK);
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut outputs = vec![SignalBuffer::control(BLOCK), SignalBuffer::control(BLOCK)];
        let (mut out, mut trig) = (Vec::new(), Vec::new());
        for (index, chunk) in pitch.chunks(BLOCK).enumerate() {
            let mut input = SignalBuffer::control(BLOCK);
            input.samples[..chunk.len()].copy_from_slice(chunk);
            let mut cv = SignalBuffer::unconnected(BLOCK, SignalType::Control);
            if let Some(transpose) = transpose_cv {
                cv = SignalBuffer::control(BLOCK);
                let start = index * BLOCK;
                cv.samples[..chunk.len()].copy_from_slice(&transpose[start..start + chunk.len()]);
            }
            q.process(&[&input, &cv], &mut outputs, params, &ctx);
            out.extend_from_slice(&outputs[0].samples[..chunk.len()]);
            trig.extend_from_slice(&outputs[1].samples[..chunk.len()]);
        }
        (out, trig)
    }

    fn run(q: &mut Quantizer, pitch: &[f32], params: &[f32]) -> (Vec<f32>, Vec<f32>) {
        run_with(q, pitch, None, params)
    }

    /// Out as whole semitones from C4.
    fn notes(out: &[f32]) -> Vec<i32> {
        out.iter().map(|v| (v * 12.0).round() as i32).collect()
    }

    /// How many times Trig rises.
    fn rising_edges(trig: &[f32]) -> usize {
        let mut previous = 0.0;
        trig.iter()
            .filter(|&&t| {
                let rising = t >= 0.5 && previous < 0.5;
                previous = t;
                rising
            })
            .count()
    }

    /// The distinct notes Out visits on a slow sweep across `octaves`
    /// (from C4), as pitch classes in the order first seen.
    fn notes_reached(root: usize, scale: usize) -> Vec<i32> {
        let mut q = Quantizer::new();
        let sweep: Vec<f32> = (0..12_000).map(|i| -0.1 + 1.2 * i as f32 / 12_000.0).collect();
        let (out, _) = run(&mut q, &sweep, &params(root, scale, 0.0));
        let mut seen = Vec::new();
        for note in notes(&out) {
            let class = note.rem_euclid(12);
            if !seen.contains(&class) {
                seen.push(class);
            }
        }
        seen.sort_unstable();
        seen
    }

    #[test]
    fn test_info_and_ports() {
        let q = Quantizer::new();
        assert_eq!(q.info().id, "util.quantizer");
        assert_eq!(q.info().category, ModuleCategory::Utility);
        let names: Vec<_> = q.ports().iter().map(|p| (p.name, p.signal_type)).collect();
        assert_eq!(
            names,
            [
                ("In", SignalType::Control),
                ("Transpose", SignalType::Control),
                ("Out", SignalType::Control),
                ("Trig", SignalType::Gate)
            ]
        );
        let params: Vec<_> = q.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params, ["Root", "Scale", "Transpose", "Mask"]);
    }

    #[test]
    fn test_every_scale_plays_its_own_notes() {
        let expected: [(&str, &[i32]); CUSTOM_SCALE] = [
            ("Chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
            ("Major", &[0, 2, 4, 5, 7, 9, 11]),
            ("Natural Minor", &[0, 2, 3, 5, 7, 8, 10]),
            ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
            ("Mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
            ("Harmonic Minor", &[0, 2, 3, 5, 7, 8, 11]),
            ("Pentatonic Major", &[0, 2, 4, 7, 9]),
            ("Pentatonic Minor", &[0, 3, 5, 7, 10]),
            ("Blues", &[0, 3, 5, 6, 7, 10]),
            ("Whole Tone", &[0, 2, 4, 6, 8, 10]),
        ];
        for (scale, (name, notes)) in expected.iter().enumerate() {
            assert_eq!(SCALE_NAMES[scale], *name);
            assert_eq!(notes_reached(0, scale), *notes, "{name}");
        }
    }

    #[test]
    fn test_root_rotates_the_scale() {
        // D major: D E F# G A B C#
        assert_eq!(notes_reached(2, 1), [1, 2, 4, 6, 7, 9, 11]);
        // A minor pentatonic: A C D E G
        assert_eq!(notes_reached(9, 7), [0, 2, 4, 7, 9]);
        assert_eq!(rotate_to_key(SCALE_MASKS[1], 12), SCALE_MASKS[1], "an octave up is the same key");
        assert_eq!(rotate_to_key(SCALE_MASKS[1], -10), rotate_to_key(SCALE_MASKS[1], 2));
    }

    #[test]
    fn test_snaps_to_the_nearest_note() {
        let mut q = Quantizer::new();
        // C major, from C4: a hair under E, between F and G, nearer B3 than C4, nearer C5 than D5
        let semis = [3.9, 6.4, -0.6, 12.8];
        let pitch: Vec<f32> = semis.iter().flat_map(|s| [s / 12.0; BLOCK]).collect();
        let (out, _) = run(&mut q, &pitch, &params(0, 1, 0.0));
        let held: Vec<i32> = notes(&out).chunks(BLOCK).map(|c| c[BLOCK - 1]).collect();
        assert_eq!(held, [4, 7, -1, 12]);
        // Out is exact V/Oct, the same as MIDI Note's
        assert_eq!(out[BLOCK - 1], crate::modules::MidiNote::midi_to_voct(64.0));
    }

    #[test]
    fn test_transpose_knob_and_cv() {
        // C major pentatonic, input on E4
        let pitch = vec![4.0 / 12.0; BLOCK * 2];
        let mut q = Quantizer::new();
        let (out, _) = run(&mut q, &pitch, &params(0, 6, 3.0));
        assert_eq!(notes(&out)[10], 7, "knob: E + 3");

        // A sequencer's F4 on Transpose moves the key up a fourth
        let cv = vec![5.0 / 12.0; BLOCK * 2];
        let mut q = Quantizer::new();
        let (out, _) = run_with(&mut q, &pitch, Some(&cv), &params(0, 6, 0.0));
        assert_eq!(notes(&out)[10], 9, "CV: E + 5");

        // Knob and CV add
        let mut q = Quantizer::new();
        let (out, _) = run_with(&mut q, &pitch, Some(&cv), &params(0, 6, -12.0));
        assert_eq!(notes(&out)[10], -3);
    }

    #[test]
    fn test_hysteresis_stops_chatter_on_a_slow_ramp() {
        // C major pentatonic (gap E-G, midpoint 5.5 st). A slow rise from
        // D to A with a small wobble riding on it, as from a noisy LFO
        let mut q = Quantizer::new();
        let seconds = 8.0;
        let samples = (seconds * SAMPLE_RATE) as usize;
        let ramp: Vec<f32> = (0..samples)
            .map(|i| {
                let t = i as f32 / samples as f32;
                let wobble = 0.12 * (i as f32 * 0.05).sin();
                (2.0 + 7.0 * t + wobble) / 12.0
            })
            .collect();
        let (out, trig) = run(&mut q, &ramp, &params(0, 6, 0.0));
        let mut changes: Vec<i32> = Vec::new();
        for note in notes(&out) {
            if changes.last() != Some(&note) {
                changes.push(note);
            }
        }
        assert_eq!(changes, [2, 4, 7, 9], "each boundary crossed once");
        assert_eq!(rising_edges(&trig), 4, "one Trig per note, the first included");
    }

    #[test]
    fn test_hysteresis_is_small() {
        // Past the boundary by more than the hysteresis, the note moves
        let mut q = Quantizer::new();
        let semis = [0.0, 1.0 + Quantizer::HYSTERESIS + 0.01, 1.0 - Quantizer::HYSTERESIS + 0.01];
        let pitch: Vec<f32> = semis.iter().flat_map(|s| [s / 12.0; BLOCK]).collect();
        let (out, _) = run(&mut q, &pitch, &params(0, 1, 0.0));
        let held: Vec<i32> = notes(&out).chunks(BLOCK).map(|c| c[BLOCK - 1]).collect();
        assert_eq!(held, [0, 2, 2], "C to D past the line, and D held just under it");
    }

    #[test]
    fn test_trig_fires_once_per_note_change() {
        // Hold, change, hold, change back, hold long
        let mut q = Quantizer::new();
        let semis = [0.0, 0.1, 4.0, 4.2, 0.0];
        let pitch: Vec<f32> = semis.iter().flat_map(|s| [s / 12.0; 4 * BLOCK]).collect();
        let (_, trig) = run(&mut q, &pitch, &params(0, 1, 0.0));
        assert_eq!(rising_edges(&trig), 3, "first note, E, back to C");
        let pulse = trig.iter().take_while(|&&t| t > 0.5).count();
        assert_eq!(pulse, (Quantizer::TRIG_SECONDS * SAMPLE_RATE) as usize);
        assert!(trig[4 * BLOCK - 1] == 0.0, "never high while the note holds");
    }

    #[test]
    fn test_quick_changes_each_get_an_edge() {
        // A new note every 2 ms, shorter than a pulse
        let mut q = Quantizer::new();
        let step = (0.002 * SAMPLE_RATE) as usize;
        let pitch: Vec<f32> = (0..8).flat_map(|n| vec![(n % 2) as f32 * 2.0 / 12.0; step]).collect();
        let (_, trig) = run(&mut q, &pitch, &params(0, 1, 0.0));
        assert_eq!(rising_edges(&trig), 8);
    }

    #[test]
    fn test_transpose_change_is_a_new_note() {
        let pitch = vec![0.0; BLOCK * 4];
        let cv: Vec<f32> = (0..BLOCK * 4).map(|i| if i < BLOCK * 2 { 0.0 } else { 2.0 / 12.0 }).collect();
        let mut q = Quantizer::new();
        let (_, trig) = run_with(&mut q, &pitch, Some(&cv), &params(0, 1, 0.0));
        assert_eq!(rising_edges(&trig), 2);
    }

    #[test]
    fn test_custom_mask_and_empty_scale() {
        let mut q = Quantizer::new();
        // Custom: root and fifth only, in G
        let custom = mask(&[0, 7]) as f32;
        let sweep: Vec<f32> = (0..4800).map(|i| i as f32 / 4800.0).collect();
        let (out, _) = run(&mut q, &sweep, &[7.0, CUSTOM_SCALE as f32, 0.0, custom]);
        assert!(notes(&out).iter().all(|n| [2, 7].contains(&n.rem_euclid(12))));

        // Nothing allowed: the pitch passes through
        let mut q = Quantizer::new();
        let (out, trig) = run(&mut q, &sweep, &[0.0, CUSTOM_SCALE as f32, 2.0, 0.0]);
        assert!((out[1000] - (sweep[1000] + 2.0 / 12.0)).abs() < 1e-6);
        assert_eq!(rising_edges(&trig), 0);
    }

    #[test]
    fn test_scale_change_moves_a_held_note_at_once() {
        // Held on E in C major; in C minor E is out, so it moves now, hysteresis or not
        let mut q = Quantizer::new();
        let pitch = vec![4.0 / 12.0; BLOCK];
        run(&mut q, &pitch, &params(0, 1, 0.0));
        let (out, _) = run(&mut q, &pitch, &params(0, 2, 0.0));
        assert_eq!(notes(&out)[0], 3);
    }

    #[test]
    fn test_poly_voices_quantize_on_their_own() {
        let mut poly = Poly::new(Quantizer::new);
        poly.prepare(SAMPLE_RATE, BLOCK);
        let ctx = ProcessContext::new(SAMPLE_RATE, BLOCK);
        let mut input = SignalBuffer::polyphonic(BLOCK, SignalType::Control);
        input.set_channels(3);
        for (channel, semis) in [0.9, 3.6, 6.8].iter().enumerate() {
            input.channel_mut(channel).fill(semis / 12.0);
        }
        let cv = SignalBuffer::unconnected(BLOCK, SignalType::Control);
        let mut outputs = vec![
            SignalBuffer::polyphonic(BLOCK, SignalType::Control),
            SignalBuffer::polyphonic(BLOCK, SignalType::Gate),
        ];
        poly.process(&[&input, &cv], &mut outputs, &params(0, 1, 0.0), &ctx);
        assert_eq!(outputs[0].channels(), 3);
        let chord: Vec<i32> = (0..3).map(|c| (outputs[0].voice(c).samples[10] * 12.0).round() as i32).collect();
        assert_eq!(chord, [0, 4, 7], "C E G");
    }

    #[test]
    fn test_reset_forgets_the_note() {
        let mut q = Quantizer::new();
        run(&mut q, &[0.3; BLOCK], &params(0, 1, 0.0));
        q.reset();
        assert_eq!(q.note, None);
        assert_eq!(q.out_note, None);
    }
}
