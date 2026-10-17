//! Mix module.
//!
//! Four signals added into one, each at its own level. It's the plain sum a
//! patch reaches for most: two envelopes into one filter's cutoff, two noise
//! colours into one VCA, a pad's oscillators into one filter. Audio and
//! control both patch in, so the same module mixes sound or modulation.
//!
//! A polyphonic cable stays one, each voice its own sum, unless Sum Voices
//! adds its voices together too: a chord's saws into one filter that drives
//! them as one sound, the way four oscillators into one filter do.
//!
//! For placing sounds left and right, sending them to a shared reverb, or
//! muting and soloing them as you play, use the Mixer.

use crate::dsp::{
    connected_input,
    context::ProcessContext,
    module_trait::{DspModule, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::{SignalBuffer, MAX_CHANNELS},
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

use super::mixer::soft_clip;

/// The number of inputs.
pub const INPUTS: usize = 4;

/// The module's ID, as patches name it.
pub const ID: &str = "util.mix";

/// A four-input mono mixer.
///
/// # Ports
///
/// **Inputs:**
/// - **In 1** to **In 4** (Audio): The signals to add. Control signals
///   patch in too.
///
/// **Outputs:**
/// - **Out** (Audio): Each input at its level, added together. Past full
///   scale the sum bends over softly toward ±1.5 instead of clipping.
///
/// # Parameters
///
/// - **Level 1** to **Level 4** (0 to 1): Each input's level. Default 1.
/// - **Sum Voices** (toggle): Adds a polyphonic cable's voices into one
///   channel too. Off, a polyphonic cable stays one, each voice its own sum,
///   and a mono input joins every voice.
pub struct Mix {
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
    /// Each Level knob, smoothed per sample.
    levels: [SmoothedValue; INPUTS],
}

impl Mix {
    /// Creates a new Mix.
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        let input = |id, name, description| PortDefinition::input_with_default(id, name, SignalType::Audio, 0.0).describe(description);
        let level = |id, name, description| {
            ParameterDefinition::new(id, name, 0.0, 1.0, 1.0, ParameterDisplay::linear("")).describe(description)
        };
        Self {
            ports: vec![
                input("in1", "In 1", "First signal to add. Audio or control"),
                input("in2", "In 2", "Second signal to add"),
                input("in3", "In 3", "Third signal to add"),
                input("in4", "In 4", "Fourth signal to add"),
                PortDefinition::output("out", "Out", SignalType::Audio)
                    .describe("Every input at its level, added together; a hot sum bends softly instead of clipping"),
            ],
            parameters: vec![
                level("level1", "Level 1", "How much of In 1 joins the sum"),
                level("level2", "Level 2", "How much of In 2 joins the sum"),
                level("level3", "Level 3", "How much of In 3 joins the sum"),
                level("level4", "Level 4", "How much of In 4 joins the sum"),
                ParameterDefinition::toggle("sum_voices", "Sum Voices", false)
                    .describe("Adds a chord's voices into one sound as well; off, each voice of a polyphonic cable is mixed on its own"),
            ],
            levels: std::array::from_fn(|_| SmoothedValue::with_default_smoothing(1.0, sample_rate)),
        }
    }
}

impl Mix {
    const PARAM_SUM_VOICES: usize = INPUTS;
}

impl Default for Mix {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for Mix {
    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Mix",
            category: ModuleCategory::Utility,
            description: "Adds four signals, audio or CV, into one",
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
        for level in &mut self.levels {
            level.set_sample_rate(sample_rate);
        }
    }

    fn process(
        &mut self,
        inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let Some(out) = outputs.first_mut() else {
            return;
        };
        let n = context.block_size.min(out.samples.len());
        let sum_voices = params[Self::PARAM_SUM_VOICES] > 0.5;
        let patched: [Option<&SignalBuffer>; INPUTS] =
            std::array::from_fn(|k| connected_input(inputs, k).filter(|buf| buf.samples.len() >= n));

        // A voice per channel of the widest input, or one for them all
        let widest = patched.iter().flatten().map(|input| input.channels()).max().unwrap_or(1);
        let channels = if sum_voices { 1 } else { widest.min(out.max_channels()).min(MAX_CHANNELS) };
        out.set_channels(channels);
        for channel in 0..channels {
            out.channel_mut(channel)[..n].fill(0.0);
        }

        for ((k, level), input) in self.levels.iter_mut().enumerate().zip(patched) {
            level.set_target(params[k]);
            let Some(input) = input else {
                // Unpatched: the knob is simply where it was left
                level.reset(level.target());
                continue;
            };
            let voices = input.channels();
            for i in 0..n {
                let gain = level.next().clamp(0.0, 1.0);
                if sum_voices {
                    let sum: f32 = (0..voices).map(|v| input.voice(v).samples[i]).sum();
                    out.samples[i] += sum * gain;
                    continue;
                }
                // A mono input joins every voice; a narrower cable leaves the rest
                for channel in 0..channels {
                    let x = match channel {
                        _ if voices == 1 => input.samples[i],
                        c if c < voices => input.voice(c).samples[i],
                        _ => continue,
                    };
                    out.channel_mut(channel)[i] += x * gain;
                }
            }
        }

        for channel in 0..channels {
            for sample in &mut out.channel_mut(channel)[..n] {
                *sample = soft_clip(*sample);
            }
        }
    }

    fn reset(&mut self) {
        for level in &mut self.levels {
            level.reset(level.target());
        }
    }

    fn polyphonic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: usize = 256;

    fn constant(value: f32) -> SignalBuffer {
        let mut buf = SignalBuffer::audio(BLOCK);
        buf.fill(value);
        buf
    }

    /// Runs `blocks` blocks with `patched` laid on the inputs by index (the
    /// rest unpatched), returning the last sample of Out.
    fn run(mix: &mut Mix, patched: &[(usize, &SignalBuffer)], levels: &[f32; INPUTS], blocks: usize) -> f32 {
        let empty = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let mut inputs: Vec<&SignalBuffer> = vec![&empty; INPUTS];
        for &(port, buf) in patched {
            inputs[port] = buf;
        }
        let mut out = vec![SignalBuffer::audio(BLOCK)];
        let ctx = ProcessContext::new(44100.0, BLOCK);
        let params = [levels[0], levels[1], levels[2], levels[3], 0.0];
        for _ in 0..blocks {
            mix.process(&inputs, &mut out, &params, &ctx);
        }
        out[0].samples[BLOCK - 1]
    }

    /// A polyphonic cable carrying `values`, one constant per channel.
    fn chord(values: &[f32]) -> SignalBuffer {
        let mut buf = SignalBuffer::polyphonic(BLOCK, SignalType::Audio);
        buf.set_channels(values.len());
        for (channel, &value) in values.iter().enumerate() {
            buf.channel_mut(channel).fill(value);
        }
        buf
    }

    /// One block of a bass on In 1 and a three-note chord on In 2, at the
    /// given levels, with Sum Voices `sum`: each channel of Out.
    fn bass_and_chord(sum: bool) -> Vec<f32> {
        let mut mix = Mix::new();
        mix.prepare(44100.0, BLOCK);
        let (bass, notes) = (constant(0.1), chord(&[0.2, 0.3, 0.4]));
        let empty = SignalBuffer::unconnected(BLOCK, SignalType::Audio);
        let inputs = [&bass, &notes, &empty, &empty];
        let mut out = vec![SignalBuffer::polyphonic(BLOCK, SignalType::Audio)];
        let params = [1.0, 0.5, 1.0, 1.0, if sum { 1.0 } else { 0.0 }];
        mix.process(&inputs, &mut out, &params, &ProcessContext::new(44100.0, BLOCK));
        (0..out[0].channels()).map(|c| out[0].voice(c).samples[BLOCK - 1]).collect()
    }

    #[test]
    fn test_a_chord_stays_a_chord_with_the_bass_in_every_voice() {
        let voices = bass_and_chord(false);
        let expected = [0.1 + 0.1, 0.1 + 0.15, 0.1 + 0.2];
        assert_eq!(voices.len(), 3);
        for (got, want) in voices.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{voices:?}");
        }
    }

    #[test]
    fn test_sum_voices_adds_a_chord_into_one_channel() {
        // The bass once, and the chord's three notes at their level
        let voices = bass_and_chord(true);
        assert_eq!(voices.len(), 1);
        assert!((voices[0] - (0.1 + 0.5 * 0.9)).abs() < 1e-6, "{voices:?}");
    }

    #[test]
    fn test_mix_info_and_ports() {
        let mix = Mix::new();
        assert_eq!(mix.info().id, "util.mix");
        assert_eq!(mix.info().category, ModuleCategory::Utility);
        let names: Vec<_> = mix.ports().iter().map(|p| p.name).collect();
        assert_eq!(names, ["In 1", "In 2", "In 3", "In 4", "Out"]);
        assert!(mix.parameters()[..INPUTS].iter().all(|p| p.default == 1.0));
        assert_eq!(mix.parameters()[Mix::PARAM_SUM_VOICES].name, "Sum Voices");
        assert!(mix.polyphonic());
    }

    #[test]
    fn test_inputs_add_at_their_levels() {
        let mut mix = Mix::new();
        mix.prepare(44100.0, BLOCK);
        let (a, b, c) = (constant(0.5), constant(0.3), constant(-0.2));
        let out = run(&mut mix, &[(0, &a), (1, &b), (3, &c)], &[0.5, 0.25, 1.0, 1.0], 20);
        assert!((out - (0.25 + 0.075 - 0.2)).abs() < 1e-4, "got {out}");
    }

    #[test]
    fn test_level_glides_without_a_click() {
        let mut mix = Mix::new();
        mix.prepare(44100.0, BLOCK);
        let a = constant(0.8);
        run(&mut mix, &[(0, &a)], &[1.0; 4], 4);
        // Turned right down: the first block still starts near where it was
        let first = run(&mut mix, &[(0, &a)], &[0.0, 1.0, 1.0, 1.0], 1);
        assert!(first > 0.0);
        let later = run(&mut mix, &[(0, &a)], &[0.0, 1.0, 1.0, 1.0], 20);
        assert!(later.abs() < 1e-4);
    }

    #[test]
    fn test_hot_sums_bend_softly() {
        // Two full-scale envelopes open a filter further than one, but never
        // past the ceiling
        let mut mix = Mix::new();
        mix.prepare(44100.0, BLOCK);
        let env = constant(1.0);
        let out = run(&mut mix, &[(0, &env), (1, &env)], &[1.0; 4], 2);
        assert!((out - soft_clip(2.0)).abs() < 1e-6);
        assert!(out > 1.0 && out < 1.5);
    }

    #[test]
    fn test_unpatched_is_silent() {
        let mut mix = Mix::new();
        mix.prepare(44100.0, BLOCK);
        assert_eq!(run(&mut mix, &[], &[1.0; 4], 2), 0.0);
    }
}
