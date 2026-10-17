//! Audio Input module.
//!
//! Brings the input device chosen in the toolbar into the patch: a
//! microphone, a guitar or a line source, to run through the filters and
//! effects. Alongside the audio it follows how loud the input is, and opens
//! a gate when it gets loud, so a drum or a picked note can trigger
//! envelopes.

use crate::dsp::{
    context::ProcessContext,
    module_trait::{DspModule, MeterLevels, ModuleCategory, ModuleInfo},
    parameter::ParameterDefinition,
    port::PortDefinition,
    signal::SignalBuffer,
    smoothed_value::SmoothedValue,
    ParameterDisplay, SignalType,
};

/// The quietest level Follow measures. Follow reads 0 here and 1 at full
/// scale, so the range spans a whisper to a shout.
pub const FOLLOW_FLOOR_DB: f32 = -60.0;

/// How far below the Threshold the input must fall before the gate closes
/// again, so a level hovering at the threshold doesn't chatter.
pub const GATE_HYSTERESIS_DB: f32 = 6.0;

/// The Channel choice: which of the device's inputs each side hears.
pub const CHANNEL_LABELS: &[&str] = &["Stereo", "1", "2"];

/// Converts a level in dBFS to Follow's scale: 0 at the floor, 1 at 0 dBFS.
#[inline]
pub fn follow_from_db(db: f32) -> f32 {
    ((db - FOLLOW_FLOOR_DB) / -FOLLOW_FLOOR_DB).clamp(0.0, 1.0)
}

/// One-pole coefficient that moves `1 - 1/e` of the way in `ms`.
#[inline]
fn coefficient(ms: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * sample_rate).max(1.0)).exp()
}

/// The module's ID, as patches name it.
pub const ID: &str = "source.audio_input";

/// The patch's audio input.
///
/// # Ports
///
/// **Outputs:**
/// - **L**, **R** (Audio): The input, after Gain. In Stereo, input 1 on L
///   and input 2 on R; on 1 or 2, that input on both. A mono device is
///   heard on both either way.
/// - **Follow** (Control): How loud the input is, on a dB scale: 0 at
///   -60 dB, 1 at full scale.
/// - **Gate** (Gate): High while the input is above the Threshold.
///
/// # Parameters
///
/// - **Gain** (-24 to +24 dB): Level of the input.
/// - **Threshold** (-60 to 0 dB): Where the gate opens.
/// - **Attack** (0.1-100 ms): How fast Follow rises.
/// - **Release** (5-2000 ms): How fast Follow falls.
/// - **Channel** (Stereo, 1, 2): Which input the module hears.
pub struct AudioInput {
    /// The Gain knob as a linear gain, smoothed.
    gain: SmoothedValue,
    /// The follower's level, linear.
    envelope: f32,
    gate_open: bool,
    /// Peak of each side since the meters were last read.
    peaks: [f32; 2],
    sample_rate: f32,
    ports: Vec<PortDefinition>,
    parameters: Vec<ParameterDefinition>,
}

impl AudioInput {
    pub fn new() -> Self {
        let sample_rate = 44100.0;
        Self {
            gain: SmoothedValue::with_default_smoothing(1.0, sample_rate),
            envelope: 0.0,
            gate_open: false,
            peaks: [0.0; 2],
            sample_rate,
            ports: vec![
                PortDefinition::output("left", "L", SignalType::Audio)
                    .describe("Input 1, or the chosen Channel, after Gain"),
                PortDefinition::output("right", "R", SignalType::Audio)
                    .describe("Input 2, or the chosen Channel, after Gain"),
                PortDefinition::output("follow", "Follow", SignalType::Control)
                    .describe("How loud the input is: 0 at -60 dB up to 1 at full scale"),
                PortDefinition::output("gate", "Gate", SignalType::Gate)
                    .describe("High while the input is louder than the Threshold"),
            ],
            parameters: vec![
                ParameterDefinition::new("gain", "Gain", -24.0, 24.0, 0.0, ParameterDisplay::linear("dB"))
                    .describe("Level of the input, before everything else"),
                ParameterDefinition::new("threshold", "Threshold", FOLLOW_FLOOR_DB, 0.0, -30.0, ParameterDisplay::linear("dB"))
                    .describe("How loud the input must get to open the gate"),
                ParameterDefinition::new("attack", "Attack", 0.1, 100.0, 5.0, ParameterDisplay::logarithmic("ms"))
                    .describe("How fast Follow rises when the input gets louder"),
                ParameterDefinition::new("release", "Release", 5.0, 2000.0, 150.0, ParameterDisplay::logarithmic("ms"))
                    .describe("How fast Follow falls when the input gets quieter"),
                ParameterDefinition::choice("channel", "Channel", CHANNEL_LABELS, 0)
                    .describe("Stereo hears inputs 1 and 2 on L and R; 1 or 2 hears that input alone, on both"),
            ],
        }
    }

    const PARAM_GAIN: usize = 0;
    const PARAM_THRESHOLD: usize = 1;
    const PARAM_ATTACK: usize = 2;
    const PARAM_RELEASE: usize = 3;
    const PARAM_CHANNEL: usize = 4;

    /// Follow's reading of the envelope.
    #[inline]
    fn follow(&self) -> f32 {
        follow_from_db(20.0 * self.envelope.max(1e-9).log10())
    }
}

impl Default for AudioInput {
    fn default() -> Self {
        Self::new()
    }
}

impl DspModule for AudioInput {
    fn is_live_input(&self) -> bool {
        true
    }

    fn info(&self) -> &ModuleInfo {
        static INFO: ModuleInfo = ModuleInfo {
            id: ID,
            name: "Audio Input",
            category: ModuleCategory::Source,
            description: "A microphone, guitar or line source from the Input device, with an envelope follower and a gate",
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
        self.gain.set_sample_rate(sample_rate);
    }

    fn process(
        &mut self,
        _inputs: &[&SignalBuffer],
        outputs: &mut [SignalBuffer],
        params: &[f32],
        context: &ProcessContext,
    ) {
        let [left_out, right_out, follow_out, gate_out, ..] = outputs else {
            return;
        };
        let gain_db = params[Self::PARAM_GAIN].clamp(-24.0, 24.0);
        self.gain.set_target(10f32.powf(gain_db / 20.0));
        let open_at = follow_from_db(params[Self::PARAM_THRESHOLD]);
        let close_at = open_at - GATE_HYSTERESIS_DB / -FOLLOW_FLOOR_DB;
        let attack = coefficient(params[Self::PARAM_ATTACK], self.sample_rate);
        let release = coefficient(params[Self::PARAM_RELEASE], self.sample_rate);
        let channel = params[Self::PARAM_CHANNEL].round() as usize;

        for i in 0..context.block_size {
            let gain = self.gain.next();
            let (one, two) = context.input.frame(i);
            let (left, right) = match channel {
                1 => (one, one),
                2 => (two, two),
                _ => (one, two),
            };
            let (left, right) = (left * gain, right * gain);
            left_out.samples[i] = left;
            right_out.samples[i] = right;
            self.peaks[0] = self.peaks[0].max(left.abs());
            self.peaks[1] = self.peaks[1].max(right.abs());

            // The louder side; on 1 or 2 both sides are that input
            let level = left.abs().max(right.abs());
            let rate = if level > self.envelope { attack } else { release };
            self.envelope += rate * (level - self.envelope);

            let follow = self.follow();
            if self.gate_open {
                self.gate_open = follow >= close_at;
            } else {
                self.gate_open = follow >= open_at && follow > 0.0;
            }
            follow_out.samples[i] = follow;
            gate_out.samples[i] = if self.gate_open { 1.0 } else { 0.0 };
        }
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.gate_open = false;
        self.peaks = [0.0; 2];
        self.gain.reset(self.gain.target());
    }

    /// The peak of L and R after Gain.
    fn take_meter_levels(&mut self) -> Option<MeterLevels> {
        let mut levels = MeterLevels::default();
        levels.peaks[0] = std::mem::take(&mut self.peaks[0]);
        levels.peaks[1] = std::mem::take(&mut self.peaks[1]);
        Some(levels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::InputAudio;

    const SR: f32 = 48000.0;
    const BLOCK: usize = 240; // 5 ms

    /// Default parameters: 0 dB gain, -30 dB threshold, 5 ms attack, 150 ms
    /// release, Stereo.
    const DEFAULTS: [f32; 5] = [0.0, -30.0, 5.0, 150.0, 0.0];

    /// The defaults with Channel set to `index` (0 Stereo, 1, 2).
    fn on_channel(index: f32) -> [f32; 5] {
        let mut params = DEFAULTS;
        params[4] = index;
        params
    }

    struct Run {
        left: Vec<f32>,
        right: Vec<f32>,
        follow: Vec<f32>,
        gate: Vec<f32>,
    }

    /// Plays stereo input through a fresh module, block by block.
    fn run(left: &[f32], right: &[f32], params: [f32; 5]) -> Run {
        let mut module = AudioInput::new();
        module.prepare(SR, BLOCK);
        module.gain.set_immediate(10f32.powf(params[0] / 20.0));
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let mut out = Run { left: vec![], right: vec![], follow: vec![], gate: vec![] };
        for (l, r) in left.chunks(BLOCK).zip(right.chunks(BLOCK)) {
            let context = ProcessContext::new(SR, l.len()).with_input(InputAudio { left: l, right: r });
            module.process(&[], &mut outputs, &params, &context);
            out.left.extend_from_slice(&outputs[0].samples[..l.len()]);
            out.right.extend_from_slice(&outputs[1].samples[..l.len()]);
            out.follow.extend_from_slice(&outputs[2].samples[..l.len()]);
            out.gate.extend_from_slice(&outputs[3].samples[..l.len()]);
        }
        out
    }

    /// A 1 kHz tone at `db` dBFS, `seconds` long.
    fn tone(db: f32, seconds: f32) -> Vec<f32> {
        let amplitude = 10f32.powf(db / 20.0);
        (0..(seconds * SR) as usize)
            .map(|n| amplitude * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin())
            .collect()
    }

    /// Silence, then a burst at `db`, then silence: 0.2 s, 0.3 s, 0.5 s.
    fn burst(db: f32) -> Vec<f32> {
        let mut signal = vec![0.0; (0.2 * SR) as usize];
        signal.extend(tone(db, 0.3));
        signal.extend(vec![0.0; (0.5 * SR) as usize]);
        signal
    }

    fn ms(samples: usize) -> f32 {
        samples as f32 / SR * 1000.0
    }

    #[test]
    fn test_ports_and_parameters() {
        let module = AudioInput::new();
        assert_eq!(module.info().id, "source.audio_input");
        assert_eq!(module.info().category, ModuleCategory::Source);
        let ports = module.ports();
        let names: Vec<_> = ports.iter().map(|p| p.name).collect();
        assert_eq!(names, ["L", "R", "Follow", "Gate"]);
        assert!(ports.iter().all(|p| p.is_output()));
        assert_eq!(ports[2].signal_type, SignalType::Control);
        assert_eq!(ports[3].signal_type, SignalType::Gate);
        let params: Vec<_> = module.parameters().iter().map(|p| p.name).collect();
        assert_eq!(params, ["Gain", "Threshold", "Attack", "Release", "Channel"]);
        // Stereo by default, so patches saved before Channel sound the same
        let channel = &module.parameters()[4];
        assert_eq!(channel.default, 0.0);
        assert!(matches!(channel.display, ParameterDisplay::Discrete { labels } if labels == CHANNEL_LABELS));
        assert!(!module.polyphonic());
    }

    #[test]
    fn test_passes_input_through_with_gain() {
        let left = tone(-12.0, 0.1);
        let right: Vec<f32> = left.iter().map(|s| -s).collect();
        let out = run(&left, &right, DEFAULTS);
        assert_eq!(out.left, left);
        assert_eq!(out.right, right);

        let louder = run(&left, &right, [6.0, -30.0, 5.0, 150.0, 0.0]);
        let ratio = louder.left[100] / left[100];
        assert!((ratio - 10f32.powf(6.0 / 20.0)).abs() < 1e-4, "ratio {ratio}");
    }

    #[test]
    fn test_no_input_is_silence() {
        let mut module = AudioInput::new();
        module.prepare(SR, BLOCK);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        for output in &mut outputs {
            output.samples.fill(1.0);
        }
        module.process(&[], &mut outputs, &DEFAULTS, &ProcessContext::new(SR, BLOCK));
        assert!(outputs.iter().all(|o| o.samples.iter().all(|&s| s == 0.0)));
    }

    #[test]
    fn test_follow_reads_the_level_on_a_db_scale() {
        // A steady sine's peak level, once settled
        for (db, expected) in [(0.0, 1.0), (-6.0, 0.9), (-30.0, 0.5), (-60.0, 0.0)] {
            let signal = tone(db, 0.5);
            let out = run(&signal, &signal, DEFAULTS);
            let settled = &out.follow[out.follow.len() - 4800..];
            let mean = settled.iter().sum::<f32>() / settled.len() as f32;
            // The follower ripples a little at 1 kHz with a 150 ms release
            assert!((mean - expected).abs() < 0.03, "{db} dB reads {mean}, expected {expected}");
        }
    }

    #[test]
    fn test_follow_attacks_and_releases_at_their_times() {
        // A steady level, so the time constants read exactly: a tone only
        // pushes the follower up on its peaks
        let onset = (0.2 * SR) as usize;
        let end = (0.5 * SR) as usize;
        let signal: Vec<f32> = (0..(1.0 * SR) as usize).map(|i| if (onset..end).contains(&i) { 0.5 } else { 0.0 }).collect();
        let out = run(&signal, &signal, DEFAULTS);
        let settled = out.follow[end - 1];
        assert!((settled - follow_from_db(-6.02)).abs() < 1e-3, "settled at {settled}");

        // Rising: 63% of the way (to 0.316, -10 dBFS) in the 5 ms attack
        let risen = out.follow[onset..].iter().position(|&f| f >= follow_from_db(-10.0)).unwrap();
        assert!((ms(risen) - 5.0).abs() < 0.2, "rose in {} ms", ms(risen));

        // Falling: down 63% (8.7 dB) in the 150 ms release
        let fallen = out.follow[end..].iter().position(|&f| f <= settled - 8.69 / 60.0).unwrap();
        assert!((ms(fallen) - 150.0).abs() < 1.0, "fell in {} ms", ms(fallen));
    }

    #[test]
    fn test_follow_tracks_a_tone_from_its_first_cycles() {
        // A peak-weighted follower on a tone: a few cycles of the 5 ms
        // attack bring it within 3 dB of the tone's peak
        let signal = burst(-6.0);
        let out = run(&signal, &signal, DEFAULTS);
        let onset = (0.2 * SR) as usize;
        let risen = out.follow[onset..].iter().position(|&f| f >= follow_from_db(-9.0)).unwrap();
        assert!(ms(risen) < 20.0, "rose in {} ms", ms(risen));
    }

    #[test]
    fn test_gate_opens_on_a_burst_and_closes_after() {
        let signal = burst(-12.0);
        let out = run(&signal, &signal, DEFAULTS);
        let onset = (0.2 * SR) as usize;
        let end = (0.5 * SR) as usize;

        assert!(out.gate[..onset].iter().all(|&g| g == 0.0), "closed before the burst");
        let opened = out.gate[onset..].iter().position(|&g| g == 1.0).expect("the gate opened");
        assert!(ms(opened) < 2.0, "opened {} ms in", ms(opened));
        assert!(out.gate[onset + opened..end].iter().all(|&g| g == 1.0), "stayed open through the burst");

        // Released from -12 dB, it closes at -36 dB (6 dB under the -30 dB
        // threshold): 24 dB down, or 2.76 time constants of 150 ms
        let closed = out.gate[end..].iter().position(|&g| g == 0.0).expect("the gate closed");
        assert!((ms(closed) - 414.0).abs() < 25.0, "closed {} ms after", ms(closed));
        assert!(out.gate[end + closed..].iter().all(|&g| g == 0.0), "stayed closed");
    }

    #[test]
    fn test_gate_ignores_input_under_the_threshold() {
        let signal = burst(-40.0);
        let out = run(&signal, &signal, DEFAULTS);
        assert!(out.gate.iter().all(|&g| g == 0.0));
        assert!(out.follow.iter().any(|&f| f > 0.3), "Follow still hears it");
    }

    #[test]
    fn test_gate_does_not_chatter_at_the_threshold() {
        // A level wobbling ±3 dB around the threshold, 20 times a second
        let n = (2.0 * SR) as usize;
        let signal: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let db = -30.0 + 3.0 * (2.0 * std::f32::consts::PI * 20.0 * t).sin();
                10f32.powf(db / 20.0) * (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
            })
            .collect();
        let out = run(&signal, &signal, DEFAULTS);
        let edges = out.gate.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(edges <= 1, "the gate switched {edges} times");
    }

    #[test]
    fn test_gain_raises_the_input_over_the_threshold() {
        let signal = burst(-40.0);
        let out = run(&signal, &signal, [18.0, -30.0, 5.0, 150.0, 0.0]);
        assert!(out.gate.contains(&1.0), "+18 dB lifts -40 dB over -30 dB");
    }

    #[test]
    fn test_follows_the_louder_side() {
        let loud = tone(-6.0, 0.3);
        let quiet = vec![0.0; loud.len()];
        let left = run(&loud, &quiet, DEFAULTS);
        let right = run(&quiet, &loud, DEFAULTS);
        assert_eq!(left.follow, right.follow);
    }

    #[test]
    fn test_channel_picks_which_input_both_sides_hear() {
        // A guitar in input 1, nothing in input 2
        let guitar = tone(-12.0, 0.3);
        let empty = vec![0.0; guitar.len()];

        let stereo = run(&guitar, &empty, on_channel(0.0));
        assert_eq!(stereo.left, guitar);
        assert_eq!(stereo.right, empty);

        let one = run(&guitar, &empty, on_channel(1.0));
        assert_eq!(one.left, guitar);
        assert_eq!(one.right, guitar);

        let two = run(&guitar, &empty, on_channel(2.0));
        assert!(two.left.iter().chain(&two.right).all(|&s| s == 0.0));
    }

    #[test]
    fn test_stereo_channel_matches_the_input_exactly() {
        let left = tone(-6.0, 0.2);
        let right: Vec<f32> = tone(-18.0, 0.2).iter().map(|s| -s).collect();
        let out = run(&left, &right, on_channel(0.0));
        assert_eq!((out.left, out.right), (left, right));
    }

    #[test]
    fn test_follow_and_gate_hear_the_chosen_channel_only() {
        // A loud burst in input 2 while input 1 stays quiet
        let quiet = burst(-50.0);
        let loud = burst(-6.0);

        let one = run(&quiet, &loud, on_channel(1.0));
        assert!(one.gate.iter().all(|&g| g == 0.0), "input 1 never crosses the threshold");
        assert!(one.follow.iter().all(|&f| f < follow_from_db(-45.0)));

        let two = run(&quiet, &loud, on_channel(2.0));
        assert!(two.gate.contains(&1.0), "input 2 opens the gate");
        // The same as hearing input 2 on both sides
        assert_eq!(two.follow, run(&loud, &loud, DEFAULTS).follow);
    }

    #[test]
    fn test_meters_report_peaks_after_gain_and_reset() {
        let mut module = AudioInput::new();
        module.prepare(SR, BLOCK);
        let mut outputs: Vec<SignalBuffer> = (0..4).map(|_| SignalBuffer::audio(BLOCK)).collect();
        let (left, right) = (vec![0.5; BLOCK], vec![-0.25; BLOCK]);
        let context = ProcessContext::new(SR, BLOCK).with_input(InputAudio { left: &left, right: &right });
        module.process(&[], &mut outputs, &DEFAULTS, &context);
        let levels = module.take_meter_levels().unwrap();
        assert_eq!(&levels.peaks[..2], &[0.5, 0.25]);
        assert_eq!(module.take_meter_levels().unwrap().peaks[0], 0.0);
    }

    #[test]
    fn test_registry_instantiation() {
        let registry = crate::engine::create_module_registry();
        let module = registry.create("source.audio_input").expect("Audio Input is registered");
        assert_eq!(module.info().name, "Audio Input");
    }
}
