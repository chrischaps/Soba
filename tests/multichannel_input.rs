//! Every input of an interface (#450): a mixer like a Tascam Model 12 gives
//! the computer each of its channels as an input of its own, and each Audio
//! Input module hears the channels its Channel picks.
//!
//! Each test gives every channel its own tone (channel `c`, counted from 1,
//! at `100 c` Hz) and listens for which tones come out of each side.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use soba::dsp::input_channels::InputChannels;
use soba::engine::{input_channel_converting, read_input_at, read_wav, read_wav_at, AudioProcessor, EngineChannels, EngineCommand};
use soba::persistence::{ConnectionData, NamedParameter, NodeData, ParameterValue, Patch};

const RATE: u32 = 48000;

/// Channel `c`'s tone (from 0): `100 (c + 1)` Hz at 0.2.
fn tone(channel: usize, frame: usize) -> f32 {
    0.2 * (TAU * 100.0 * (channel + 1) as f32 * frame as f32 / RATE as f32).sin()
}

/// `frames` interleaved frames of `channels` channels, each its own tone,
/// from frame `start`.
fn mixer(start: usize, frames: usize, channels: usize) -> Vec<f32> {
    (start..start + frames).flat_map(|n| (0..channels).map(move |c| tone(c, n))).collect()
}

/// How loud `hz` is in `signal`: the amplitude of its sine.
fn level_at(signal: &[f32], hz: f32) -> f32 {
    let (mut sin, mut cos) = (0.0f64, 0.0f64);
    for (n, &s) in signal.iter().enumerate() {
        let phase = (TAU * hz * n as f32 / RATE as f32) as f64;
        sin += s as f64 * phase.sin();
        cos += s as f64 * phase.cos();
    }
    (2.0 * (sin * sin + cos * cos).sqrt() / signal.len() as f64) as f32
}

/// Which channels' tones (from 1) are in `signal`, louder than 0.01.
fn tones_in(signal: &[f32], channels: usize) -> Vec<usize> {
    (1..=channels).filter(|&c| level_at(signal, 100.0 * c as f32) > 0.01).collect()
}

/// Two Audio Inputs, the first on `left` into the output's Left and the
/// second on `right` into its Right.
fn two_inputs(left: InputChannels, right: InputChannels) -> Patch {
    let mut patch = Patch::new("two from the mixer");
    for (id, channels) in [(1, left), (2, right)] {
        let mut input = NodeData::new(id, "source.audio_input", (0.0, 100.0 * id as f32));
        input.parameters = vec![NamedParameter::new("Channel", ParameterValue::Select(channels.index()))];
        patch.nodes.push(input);
    }
    patch.nodes.push(NodeData::new(3, "output.audio", (300.0, 0.0)));
    patch.connections.push(ConnectionData::new(1, "L", 3, "Left"));
    patch.connections.push(ConnectionData::new(2, "R", 3, "Right"));
    patch
}

#[test]
fn a_twelve_channel_device_reaches_the_audio_inputs_set_to_its_channels() {
    let (mut ui, engine) = EngineChannels::with_defaults().split();
    let mut processor = AudioProcessor::new(RATE as f32, 256, engine);
    // Audio Input 1 on channel 7 into Left, Audio Input 2 on 3+4 (its R,
    // channel 4) into Right
    for (node_id, module_id) in [(1, "source.audio_input"), (2, "source.audio_input"), (3, "output.audio")] {
        ui.send_command(EngineCommand::AddModule { node_id, module_id });
    }
    ui.send_command(EngineCommand::Connect { from_node: 1, from_port: 0, to_node: 3, to_port: 0 });
    ui.send_command(EngineCommand::Connect { from_node: 2, from_port: 1, to_node: 3, to_port: 1 });
    for (node_id, channels) in [(1, InputChannels::Mono(6)), (2, InputChannels::Pair(2))] {
        ui.send_command(EngineCommand::SetParameter { node_id, param_index: 4, value: channels.index() as f32 });
    }
    ui.send_command(EngineCommand::SetPlaying(true));

    // A synthetic twelve-channel device, 480 frames a packet
    let (mut device, feed, monitor) = input_channel_converting(RATE, RATE, 12);
    assert_eq!(feed.channels(), 12);
    ui.connect_input(feed);
    assert!(ui.flush());

    let mut output = vec![0.0; 2 * 256];
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let mut pushed = 0;
    for block in 0..(RATE as usize / 256) {
        while pushed <= block * 256 + 256 {
            device.push_f32(&mixer(pushed, 480, 12), 12);
            pushed += 480;
        }
        processor.process(&mut output, 2);
        ui.flush();
        left.extend(output.iter().step_by(2));
        right.extend(output.iter().skip(1).step_by(2));
    }
    assert_eq!(monitor.underrun_frames(), 0);

    // The second half second, long after the jitter buffer filled
    let half = left.len() / 2;
    assert_eq!(tones_in(&left[half..], 12), [7], "channel 7 alone on Left");
    assert_eq!(tones_in(&right[half..], 12), [4], "channel 4, the R of 3+4, on Right");
    // At the Output's default volume of 0.8 (by its RMS: the jitter buffer
    // settling now and then shifts the tone's phase)
    let rms = (left[half..].iter().map(|s| s * s).sum::<f32>() / (left.len() - half) as f32).sqrt();
    let level = rms * std::f32::consts::SQRT_2;
    assert!((level - 0.8 * 0.2).abs() < 0.01, "channel 7 at {level}");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soba-multichannel-input-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Writes `seconds` of the mixer's `channels` tones as a float WAV at `rate`.
fn write_mixer_wav(path: &Path, channels: usize, rate: u32, seconds: f32) {
    let spec = hound::WavSpec { channels: channels as u16, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..(seconds * rate as f32) as usize {
        for c in 0..channels {
            writer.write_sample(0.2 * (TAU * 100.0 * (c + 1) as f32 * n as f32 / rate as f32).sin()).unwrap();
        }
    }
    writer.finalize().unwrap();
}

#[test]
fn render_input_plays_each_channel_of_a_file_into_its_own_audio_input() {
    // A four-channel take of the mixer, and Audio Inputs on 3 and on 1+2
    let wav = scratch("mixer.wav");
    write_mixer_wav(&wav, 4, RATE, 1.0);
    let patch = scratch("two-inputs.json");
    let json = serde_json::to_string(&two_inputs(InputChannels::Mono(2), InputChannels::Pair(0))).unwrap();
    std::fs::write(&patch, json).unwrap();
    let out = scratch("rendered.wav");

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_render"))
        .args([patch.as_os_str(), out.as_os_str()])
        .args(["--seconds", "1", "--input"])
        .arg(&wav)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "render failed");

    let (rendered, rate) = read_wav(&out).unwrap();
    assert_eq!(rate, RATE);
    let settled = RATE as usize / 10..rendered.left.len();
    assert_eq!(tones_in(&rendered.left[settled.clone()], 4), [3], "channel 3 reaches its own Audio Input");
    assert_eq!(tones_in(&rendered.right[settled], 4), [2], "the R of 1+2");
}

#[test]
fn a_stereo_file_reads_for_audio_input_as_it_always_did() {
    // At another rate, so it's converted on the way in
    let wav = scratch("stereo.wav");
    write_mixer_wav(&wav, 2, 44100, 0.5);
    let (stereo, _) = read_wav_at(&wav, RATE).unwrap();
    let (channels, recorded) = read_input_at(&wav, RATE).unwrap();
    assert_eq!(recorded, 44100);
    assert_eq!(channels.channels(), 2);
    assert_eq!(channels.channel(0), stereo.left);
    assert_eq!(channels.channel(1), stereo.right);

    // And a twelve-channel file keeps all twelve, each its own
    let wide = scratch("twelve.wav");
    write_mixer_wav(&wide, 12, RATE, 0.25);
    let (channels, _) = read_input_at(&wide, RATE).unwrap();
    assert_eq!(channels.channels(), 12);
    assert_eq!(tones_in(&channels.channel(6), 12), [7]);
}
