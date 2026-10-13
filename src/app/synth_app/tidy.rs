//! Trim and Normalize, from a Sampler's face: the file it plays, tidied
//! into a new one beside the patch (see [`crate::dsp::tidy`]).
//!
//! The original is left as it was. The Sampler is pointed at the new file
//! as one undo step, so undo brings back the untouched recording. A trim
//! moves Start, End, the loop's markers and any slice markers placed by
//! hand with the sound, so they still mark the same moments in it. Either way the result plays once from the
//! face, as a file just opened does.

use std::sync::Arc;

use egui_node_graph2::NodeId;

use crate::dsp::tidy::{self, signed_db};
#[cfg(test)]
use crate::dsp::tidy::Level;
use crate::graph::Tidy;
use crate::persistence::sample_files;
use super::SynthApp;

/// The Sampler's markers, which a trim moves with the sound.
const MARKERS: [&str; 4] = ["Start", "End", "Loop Start", "Loop End"];

impl SynthApp {
    /// Trims or normalizes a Sampler's file into a new one, and points the
    /// Sampler at it.
    pub(super) fn tidy_sample(&mut self, node_id: NodeId, tidy: Tidy) {
        let Some(key) = self.graph_state.graph.nodes.get(node_id).and_then(|node| node.user_data.file.clone()) else { return };
        let name = sample_files::file_name(&key).to_string();
        let (source, survey) = match self.user_state.samples.load(&key) {
            Ok(shelved) => (Arc::clone(&shelved.source), shelved.survey().clone()),
            Err(e) => {
                self.status_message = Some(format!("Couldn't read {name}: {e}"));
                return;
            }
        };
        let (made, word, done) = match tidy {
            Tidy::Trim => {
                let Some(trimmed) = tidy::trim(&source, &survey) else {
                    self.status_message = Some(format!("{name} is already its sound: nothing to trim"));
                    return;
                };
                let sound = survey.sound.clone().expect("a trim has a sound");
                let rate = source.sample_rate();
                let done = format!(
                    "Trimmed {name}: {:.2} s off the start, {:.2} s off the end",
                    sound.start as f32 / rate,
                    (source.frames() - sound.end) as f32 / rate
                );
                self.move_slice_markers_with(node_id, source.frames(), sound.clone());
                self.move_markers_with(node_id, source.frames(), sound);
                (trimmed, "trimmed", done)
            }
            Tidy::Normalize(level) => {
                let Some(normalized) = tidy::normalize(&source, &survey, level) else {
                    self.status_message = Some(format!("{name} is silent: nothing to normalize"));
                    return;
                };
                let mut done = format!("Normalized {name}, {}: {}", level.describe(), signed_db(normalized.gain_db));
                if normalized.held_back_db > 0.0 {
                    done.push_str(&format!(", {:.1} dB short, held back by a peak too loud to limit (a popped plosive?)", normalized.held_back_db));
                } else if normalized.limited_db > 0.05 {
                    done.push_str(&format!(", peaks limited by up to {:.1} dB", normalized.limited_db));
                }
                (normalized.sample, "normalized", done)
            }
        };

        let stem = stem_without(&name, word);
        match self.keep_beside_patch(made, |n| if n == 1 { format!("{stem} {word}.wav") } else { format!("{stem} {word} {n}.wav") }) {
            Ok(new_key) => {
                if let Some(node) = self.graph_state.graph.nodes.get_mut(node_id) {
                    node.user_data.file = Some(new_key);
                }
                let verb = if tidy == Tidy::Trim { "Trim" } else { "Normalize" };
                self.history.name_next(format!("{verb} {name}"));
                self.status_message = Some(format!("{done}. Undo brings back the untouched file"));
                self.cue_start_to_end(node_id);
            }
            Err(e) => self.status_message = Some(format!("Couldn't write the {word} file: {e}")),
        }
    }

    /// Moves a Sampler's markers to the same moments in the recording cut
    /// to `kept` (frames of the `frames` it had): a marker in what was cut
    /// goes to that end.
    fn move_markers_with(&mut self, node_id: NodeId, frames: usize, kept: std::ops::Range<usize>) {
        let graph = &mut self.graph_state.graph;
        let Some(node) = graph.nodes.get(node_id) else { return };
        let inputs: Vec<_> = node.inputs.iter().filter(|(name, _)| MARKERS.contains(&name.as_str())).map(|(_, id)| *id).collect();
        for input in inputs {
            let value = &mut graph.inputs[input].value;
            let at = value.actual_value() as f64 * frames as f64;
            let moved = (at - kept.start as f64) / kept.len() as f64;
            // To the nearest 0.1%, as a knob sets it
            value.set_actual_value(((moved.clamp(0.0, 1.0) * 1000.0).round() / 1000.0) as f32);
        }
    }
}

/// A file's name without its extension, or the word a tidy adds to it
/// (" trimmed", " trimmed 2"), so tidying twice doesn't say it twice.
fn stem_without(name: &str, word: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let bare = stem.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
    bare.strip_suffix(&format!(" {word}")).filter(|s| !s.trim().is_empty()).unwrap_or(stem).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidied_names_say_what_was_done_once() {
        assert_eq!(stem_without("Take 1.wav", "trimmed"), "Take 1");
        assert_eq!(stem_without("Take 1 trimmed.wav", "trimmed"), "Take 1");
        assert_eq!(stem_without("Take 1 trimmed 2.wav", "trimmed"), "Take 1");
        assert_eq!(stem_without("Take 1 trimmed.wav", "normalized"), "Take 1 trimmed");
        assert_eq!(stem_without("trimmed.wav", "trimmed"), "trimmed");
        assert_eq!(stem_without("kick", "trimmed"), "kick");
        assert_eq!(stem_without("untrimmed.wav", "trimmed"), "untrimmed");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod round {
    use super::*;
    use crate::dsp::SampleData;
    use crate::modules::sampler::SAMPLER_ID;
    use crate::persistence::{save_to_file, NodeData, Patch};
    use crate::graph::SynthValueType;
    use std::path::Path;

    fn the_sampler(app: &SynthApp) -> NodeId {
        app.graph_state.graph.nodes.iter().find(|(_, n)| n.user_data.module_id == SAMPLER_ID).map(|(id, _)| id).expect("a Sampler")
    }

    fn file(app: &SynthApp) -> String {
        app.graph_state.graph.nodes[the_sampler(app)].user_data.file.clone().expect("a file")
    }

    fn knob(app: &SynthApp, name: &str) -> f32 {
        let node = &app.graph_state.graph.nodes[the_sampler(app)];
        let (_, input) = node.inputs.iter().find(|(input, _)| input == name).unwrap();
        app.graph_state.graph.get_input(*input).value.actual_value()
    }

    fn set_knob(app: &mut SynthApp, name: &str, value: f32) {
        let node = &app.graph_state.graph.nodes[the_sampler(app)];
        let (_, input) = *node.inputs.iter().find(|(input, _)| input == name).unwrap();
        if let SynthValueType::Number { value: v, .. } = &mut app.graph_state.graph.inputs[input].value {
            *v = value;
        }
    }

    /// A second of quiet room, a second of tone at −30 dBFS RMS, two of room.
    fn padded(rate: f32) -> SampleData {
        let mut seed = 7u32;
        let samples = (0..(4.0 * rate) as usize)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let room = 0.0005 * ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0);
                let t = i as f32 / rate;
                let tone = if (1.0..2.0).contains(&t) { 0.0447 * (std::f32::consts::TAU * 220.0 * t).sin() } else { 0.0 };
                room + tone
            })
            .collect();
        SampleData::mono(samples, rate)
    }

    /// Trim and Normalize from the face, on a patch beside its file: new
    /// files beside the patch, the markers moved, the original untouched,
    /// and undo bringing it back. The patch has no output, so it's silent.
    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn tidying_writes_new_files_and_undo_brings_back_the_old() {
        let mut app = SynthApp::new(false);
        let folder = std::env::temp_dir().join(format!("soba-tidy-{}", std::process::id()));
        std::fs::create_dir_all(folder.join("samples")).unwrap();
        let original = folder.join("samples").join("padded.wav");
        let bytes = sample_files::encode_wav(&padded(48000.0));
        std::fs::write(&original, &bytes).unwrap();
        let path = folder.join("Song.json");
        let mut patch = Patch::new("Song");
        let mut sampler = NodeData::new(1, SAMPLER_ID, (0.0, 0.0));
        sampler.file = Some("samples/padded.wav".to_string());
        patch.nodes.push(sampler);
        save_to_file(&patch, &path).unwrap();
        app.open_file(&path);
        app.sync_samples();
        // End at 2.5 s, in the room after the tone
        set_knob(&mut app, "End", 0.625);
        app.sync_history();
        let untouched = file(&app);

        let node_id = the_sampler(&app);
        app.tidy_sample(node_id, Tidy::Trim);
        app.sync_history();
        let trimmed_key = file(&app);
        assert_eq!(Path::new(&trimmed_key), folder.join("Song samples").join("padded trimmed.wav"));
        let trimmed = sample_files::read_wav_file(Path::new(&trimmed_key)).unwrap().sample;
        // From the tone at 1 s to 300 ms after it ends at 2 s
        assert!((trimmed.seconds() - 1.3).abs() <= 0.03, "{}", trimmed.seconds());
        // End was 2.5 s in, past what's kept: it goes to the end
        assert_eq!(knob(&app, "End"), 1.0);
        assert_eq!(knob(&app, "Start"), 0.0);
        assert_eq!(std::fs::read(&original).unwrap(), bytes, "the original is left alone");

        // Normalized to −16 dBFS RMS, within a tenth of a dB
        app.tidy_sample(node_id, Tidy::Normalize(Level::Rms));
        app.sync_history();
        let normalized_key = file(&app);
        assert_eq!(sample_files::file_name(&normalized_key), "padded trimmed normalized.wav");
        let normalized = sample_files::read_wav_file(Path::new(&normalized_key)).unwrap().sample;
        let rms = 20.0 * tidy::Survey::of(&trimmed).rms_of(&normalized).log10();
        assert!((rms - tidy::TARGET_RMS_DB).abs() < 0.1, "{rms}");

        // Undo twice: the untouched file, and End where it was
        app.undo();
        assert_eq!(file(&app), trimmed_key);
        app.undo();
        assert_eq!(file(&app), untouched);
        assert_eq!(knob(&app, "End"), 0.625);
        app.redo();
        assert_eq!(file(&app), trimmed_key);

        std::fs::remove_dir_all(&folder).ok();
    }
}
