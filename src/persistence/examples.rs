//! Example patches that ship with the app.
//!
//! The patches live in `patches/` at the repository root, where the docs
//! link to them, and are compiled into the binary so the File → Examples
//! menu works wherever the app is run from. Each recipe in the docs
//! (`docs/src/recipes/`) has a patch here that builds exactly what it
//! describes.

use super::{patch_from_json, Patch, PatchError};

/// A patch that ships with the app.
#[derive(Debug)]
pub struct Example {
    /// Name shown in the Examples menu.
    pub name: &'static str,
    /// File name in `patches/`, also offered when the example is saved.
    pub file_name: &'static str,
    /// One line on what the patch teaches, for the menu tooltip.
    pub description: &'static str,
    json: &'static str,
}

impl Example {
    /// Parses the example into a patch.
    pub fn patch(&self) -> Result<Patch, PatchError> {
        patch_from_json(self.json)
    }
}

/// A sample that ships with the examples: its path in `patches/`, and the
/// WAV, compiled in. The browser build leaves the WAV out (an empty slice)
/// so every visitor doesn't download every example's samples; it fetches
/// one from beside the page when a patch first plays it (see
/// [`super::sample_files`]).
#[cfg(not(target_arch = "wasm32"))]
macro_rules! shipped {
    ($path:literal) => {
        ($path, include_bytes!(concat!("../../patches/", $path)))
    };
}
#[cfg(target_arch = "wasm32")]
macro_rules! shipped {
    ($path:literal) => {
        ($path, &[])
    };
}

/// The samples the examples play, as (path in `patches/`, the WAV).
pub const EXAMPLE_SAMPLES: &[(&str, &[u8])] = &[
    // One strike of the FM Synthesis bell at C4, rendered by Soba
    shipped!("samples/fm-bell-c4.wav"),
    // "Hello. I am Soba. I sing in sines.", from tools/voice/speak.py's
    // formant synthesizer
    shipped!("samples/soba-speaks.wav"),
    // The same line in Chris's voice, recorded with tools/voice/record.py
    shipped!("samples/soba-speaks-chris.wav"),
    // Clair de Satellite's sung lines, twelve slots of one file, and its
    // one plain word: Chris's voice, cloned with ElevenLabs
    shipped!("samples/clair-lines.wav"),
    shipped!("samples/clair-still.wav"),
    // Held Vowels' line, "ohh", "ooh", "ee", "ahh" a bar apart, sung by
    // tools/voice/speak.py --vowels
    shipped!("samples/held-vowels.wav"),
    // Cut-Up's break: two bars of Soba's own Drums, swung, from
    // tools/breaks/make_break.py
    shipped!("samples/soba-break.wav"),
];

/// Every example, in menu order: from a first note to self-playing patches.
pub const EXAMPLES: &[Example] = &[
    Example {
        name: "First Sound",
        file_name: "first-sound.json",
        description: "Oscillator, envelope and VCA: the smallest playable voice. Press Play, then play the Z to M keys",
        json: include_str!("../../patches/first-sound.json"),
    },
    Example {
        name: "Basic Subtractive",
        file_name: "basic-subtractive.json",
        description: "A saw through a filter, with separate envelopes for brightness and volume",
        json: include_str!("../../patches/basic-subtractive.json"),
    },
    Example {
        name: "FM Synthesis",
        file_name: "fm-synthesis.json",
        description: "One sine bending another's pitch into a bell that starts bright and fades pure",
        json: include_str!("../../patches/fm-synthesis.json"),
    },
    Example {
        name: "Lush Pad",
        file_name: "lush-pad.json",
        description: "A polyphonic unison saw pad with slow attack, chorus and reverb. Hold chords",
        json: include_str!("../../patches/lush-pad.json"),
    },
    Example {
        name: "Sampled Keys",
        file_name: "sampled-keys.json",
        description: "A bell, sampled from the FM Synthesis example, played across the keyboard: a Sampler voice for every key you hold",
        json: include_str!("../../patches/sampled-keys.json"),
    },
    Example {
        name: "Live Looper",
        file_name: "live-looper.json",
        description: "Play your own instrument into a looper: record four bars on the downbeat, overdub layers over them and undo the last, through tempo-synced echo and a room",
        json: include_str!("../../patches/live-looper.json"),
    },
    Example {
        name: "Generative Ambient",
        file_name: "generative-ambient.json",
        description: "Plays itself: a slow pentatonic sequence, sample-and-hold brightness, long echoes",
        json: include_str!("../../patches/generative-ambient.json"),
    },
    Example {
        name: "Shoreline",
        file_name: "shoreline.json",
        description: "Plays itself: one Noise module makes the surf, varies every wave, and picks the notes of a glassy chime",
        json: include_str!("../../patches/shoreline.json"),
    },
    Example {
        name: "Rhythmic Sequence",
        file_name: "rhythmic-sequence.json",
        description: "Plays itself: a 16-step acid bassline with a resonant filter, drive and delay, over noise hi-hats",
        json: include_str!("../../patches/rhythmic-sequence.json"),
    },
    Example {
        name: "Backbeat",
        file_name: "backbeat.json",
        description: "Plays itself: a full drum kit from oscillators and noise, with ghost notes, choked hats, and a tom fill and crash every four bars",
        json: include_str!("../../patches/backbeat.json"),
    },
    Example {
        name: "Drum Machine",
        file_name: "drum-machine.json",
        description: "Plays itself: Backbeat's kit in 14 modules. Five Drum voices on five lanes, a choked open hat, tuned toms and a room",
        json: include_str!("../../patches/drum-machine.json"),
    },
    Example {
        name: "Roll Call",
        file_name: "roll-call.json",
        description: "Plays itself: eight drums on one Trigger Sequencer, with ratcheted rolls, ghost notes that only sometimes play, and a fill and crash every four bars from its Chain",
        json: include_str!("../../patches/roll-call.json"),
    },
    Example {
        name: "Afterglow",
        file_name: "afterglow.json",
        description: "Plays itself: a Chord Sequencer moves an arpeggio through Fmaj9, Cadd9, G6 and Am9 and plays them on a warm pad, under dotted-eighth tape echoes",
        json: include_str!("../../patches/afterglow.json"),
    },
    Example {
        name: "Front Porch",
        file_name: "front-porch.json",
        description: "Plays itself: rhythm guitar on the Electric Piano. A Chord Sequencer strums G, D/F#, Em7 and Cadd9 down and up in eighths, rolling each chord across its strings and brushing the last ones lightly",
        json: include_str!("../../patches/front-porch.json"),
    },
    Example {
        name: "Soba Speaks",
        file_name: "soba-speaks.json",
        description: "Plays itself: a Vocoder puts a spoken line on a pad of four falling chords. Mixer mutes switch the voice between a person, a robot and your own mic",
        json: include_str!("../../patches/soba-speaks.json"),
    },
    Example {
        name: "Held Vowels",
        file_name: "held-vowels.json",
        description: "Plays itself: a soft triangle pad sings ohh, ooh, ee and ahh. The Vocoder's Freeze holds each vowel through a chord and lets go for one beat to take the next, and Flatten lets a dull pad speak",
        json: include_str!("../../patches/held-vowels.json"),
    },
    Example {
        name: "Interlock",
        file_name: "interlock.json",
        description: "Plays itself: Logic splits a three-against-four rhythm between two gamelan-style parts that interlock into one fast melody, while a slow tide brings the second part in and out",
        json: include_str!("../../patches/interlock.json"),
    },
    Example {
        name: "Cut-Up",
        file_name: "cut-up.json",
        description: "Plays itself: a two-bar break, sliced, follows the Clock 16 BPM faster than it was played without going up in pitch. A Chopper rearranges it step by step: a slice reversed, one pitched down, a stutter into the top, and dice on a few",
        json: include_str!("../../patches/cut-up.json"),
    },
    Example {
        name: "From One Sine",
        file_name: "from-one-sine.json",
        description: "Plays itself, start to finish: a four-and-a-half-minute song in 77 modules, scored by three Arrangers that ride the faders section by section. A lone sine's motif grows into the whole rack, and a Looper brings it back reversed on warping tape",
        json: include_str!("../../patches/from-one-sine.json"),
    },
    Example {
        name: "Clair de Satellite",
        file_name: "clair-de-satellite.json",
        description: "Plays itself: a song in the manner of Air, scored by three Arrangers. Rhodes, string machine and a vocoder singing on the chords, each part a group, and a satellite at either end",
        json: include_str!("../../patches/clair-de-satellite.json"),
    },
];

/// The example opened when the app starts with nothing else to open.
pub fn first_sound() -> &'static Example {
    &EXAMPLES[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::analysis::{amp_to_db, peak, rms};
    use crate::engine::OfflineRenderer;
    use crate::persistence::compile_patch;

    #[test]
    fn test_every_patch_file_is_an_example() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("patches");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json"))
            .collect();
        files.sort();
        let mut listed: Vec<String> = EXAMPLES.iter().map(|e| e.file_name.to_string()).collect();
        listed.sort();
        assert_eq!(files, listed, "patches/ and EXAMPLES must list the same files");
    }

    #[test]
    fn test_examples_load_cleanly() {
        for example in EXAMPLES {
            let patch = example.patch().unwrap_or_else(|e| panic!("{}: {}", example.name, e));
            assert_eq!(patch.name, example.name);
            // Saved by the current build, so loading never has to migrate them
            assert_eq!(patch.version, patch.required_version(), "{}", example.name);
            let compiled = compile_patch(&patch).unwrap();
            assert!(compiled.warnings.is_empty(), "{}: {:?}", example.name, compiled.warnings);
        }
    }

    #[test]
    fn test_every_example_explains_itself() {
        for example in EXAMPLES {
            let patch = example.patch().unwrap();
            assert!(!patch.frames.is_empty(), "{}: no frames", example.name);
            assert!((1..=3).contains(&patch.notes.len()), "{}: {} notes", example.name, patch.notes.len());
            for frame in &patch.frames {
                assert!(!frame.title.is_empty(), "{}: an untitled frame", example.name);
                let tint = crate::graph::annotations::Tint::from_key(&frame.color);
                assert_eq!(tint.key(), frame.color, "{}: frame {} has an unknown color", example.name, frame.title);
            }
        }
    }

    /// The audition phrase, plucked: each note a decaying tone with an
    /// octave above it, as a guitar into an Audio Input might play it.
    fn plucked_audition(sample_rate: f32, seconds: f32) -> crate::engine::StereoBuffer {
        let frames = (sample_rate * seconds) as usize;
        let mut left = vec![0.0_f32; frames];
        for &(note, start, length) in crate::engine::AUDITION {
            let hz = 440.0 * 2f32.powf((note as f32 - 69.0) / 12.0);
            let from = (start * sample_rate) as usize;
            for (n, sample) in left.iter_mut().enumerate().skip(from).take((length * sample_rate) as usize) {
                let t = (n - from) as f32 / sample_rate;
                let phase = std::f32::consts::TAU * hz * t;
                *sample += 0.25 * (-3.0 * t).exp() * (phase.sin() + 0.3 * (2.0 * phase).sin());
            }
        }
        crate::engine::StereoBuffer { right: left.clone(), left }
    }

    #[test]
    fn test_examples_make_sound() {
        for example in EXAMPLES {
            let patch = example.patch().unwrap();
            let (mut renderer, compiled) = OfflineRenderer::from_patch(&patch, 48_000.0, 256).unwrap();
            let missing = renderer.load_samples(&compiled, super::super::sample_files::SampleBase::Example);
            assert!(missing.is_empty(), "{}: {:?}", example.name, missing);
            // Patches played live hear the audition phrase plucked into them
            if patch.all_nodes().iter().any(|n| n.module_id == "source.audio_input") {
                renderer.set_audio_input(plucked_audition(48_000.0, 5.0));
            }
            let audio = renderer.render_audition(&patch, &compiled, 5.0);

            for (side, channel) in [("left", &audio.left), ("right", &audio.right)] {
                assert!(
                    channel.iter().all(|s| s.is_finite()),
                    "{}: {} channel has NaN or infinite samples",
                    example.name,
                    side
                );
                // Sound, not silence: how loud is tests/example_loudness.rs's
                // job, and some examples open quietly (Clair de Satellite on
                // a satellite over tape hiss)
                let (peak_db, rms_db) = (amp_to_db(peak(channel)), amp_to_db(rms(channel)));
                assert!(rms_db > -40.0, "{}: {} channel too quiet, {:.1} dBFS RMS", example.name, side, rms_db);
                assert!(peak_db < 0.0, "{}: {} channel clips, {:.1} dBFS peak", example.name, side, peak_db);
            }
        }
    }
}
