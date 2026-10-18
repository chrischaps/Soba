//! Sample files: reading WAV, MP3, FLAC, Ogg Vorbis and Ogg Opus files into [`SampleData`],
//! and where a patch's samples are.
//!
//! In the editor, a Sampler names its file by a *key*: an absolute path, or
//! `example:samples/name.wav` for a sample that ships inside the app with
//! the example patches. A patch file stores the path relative to itself
//! when the sample sits beside or below it, and absolute otherwise, so a
//! patch folder can be moved or zipped and shared whole. [`resolve`] turns
//! a saved path back into a key, and [`to_patch_path`] goes the other way.
//!
//! In the browser the examples' samples aren't inside the app: each is
//! fetched from beside the page the first time it's loaded, and reads as
//! [`LOADING`] until it arrives.

use std::io::{Read, SeekFrom};
use std::path::{Path, PathBuf};

use symphonia::core::io::MediaSource;

use crate::dsp::{SampleData, MAX_SAMPLE_SECONDS};

use super::examples::EXAMPLE_SAMPLES;

/// What keys for the samples that ship with the examples start with.
pub const EXAMPLE_PREFIX: &str = "example:";

/// What keys for files kept only in the browser's tab start with: a file
/// dropped there, or a take recorded or tidied there.
pub const DROPPED_PREFIX: &str = "dropped:";

/// Why [`load`] has no recording yet for a shipped sample the browser is
/// still fetching: ask again later, rather than report it.
pub const LOADING: &str = "loading";

/// Whether a load failed only because the sample is still on its way.
pub fn is_loading(problem: &str) -> bool {
    problem == LOADING
}

/// The extensions of the files a Sampler plays.
pub const EXTENSIONS: &[&str] = &["wav", "wave", "mp3", "flac", "ogg", "oga", "opus"];

/// Whether a file's name says it's one a Sampler plays.
pub fn is_sound_file(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A decoded sound file.
#[derive(Debug)]
pub struct Decoded {
    pub sample: SampleData,
    /// Whether it was longer than [`MAX_SAMPLE_SECONDS`] and was cut.
    pub truncated: bool,
}

/// Decodes a WAV file: integer or float, mono, stereo, or more (which
/// keeps its first two channels). Anything past [`MAX_SAMPLE_SECONDS`] is
/// left unread.
pub fn decode_wav(reader: impl Read) -> Result<Decoded, String> {
    let mut reader = hound::WavReader::new(reader).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let rate = spec.sample_rate.max(1);
    let total = reader.duration() as usize;
    let keep = total.min((MAX_SAMPLE_SECONDS * rate as f32) as usize);
    let wanted = keep * channels;

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().take(wanted).collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
            reader.samples::<i32>().take(wanted).map(|s| s.map(|s| s as f32 * scale)).collect::<Result<_, _>>()
        }
    }
    .map_err(|e| e.to_string())?;

    let frames = samples.len() / channels;
    let sample = if channels == 1 {
        SampleData::mono(samples, rate as f32)
    } else {
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);
        for frame in samples.chunks_exact(channels) {
            left.push(frame[0]);
            right.push(frame[1]);
        }
        SampleData::stereo(left, right, rate as f32)
    };
    Ok(Decoded { sample, truncated: keep < total })
}

/// Decodes a WAV, MP3, FLAC or Ogg file, telling a WAV by its first bytes
/// and the others by what they hold. `name` is a hint, for an MP3 without
/// tags at its start.
pub fn decode(mut source: impl MediaSource + 'static, name: &str) -> Result<Decoded, String> {
    let mut magic = [0u8; 4];
    let riff = source.read_exact(&mut magic).is_ok() && &magic == b"RIFF";
    source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    if riff {
        decode_wav(std::io::BufReader::new(source))
    } else {
        decode_compressed(Box::new(source), name)
    }
}

/// Decodes an MP3, FLAC or Ogg (Vorbis or Opus) file. Mono stays mono, and more than one
/// channel keeps the first two. Anything past [`MAX_SAMPLE_SECONDS`] is
/// left unread, and a damaged frame is skipped rather than failing the file.
fn decode_compressed(source: Box<dyn MediaSource>, name: &str) -> Result<Decoded, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let mut hint = Hint::new();
    if let Some(extension) = Path::new(name).extension() {
        hint.with_extension(&extension.to_string_lossy());
    }
    let stream = MediaSourceStream::new(source, Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| "not a WAV, MP3, FLAC or Ogg file".to_string())?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio in the file")?;
    let track_id = track.id;
    // Where it says how long it is: an Ogg's last packet decodes past its end
    let length = track.num_frames.map(|n| n as usize);
    let not_ours = || "its audio isn't MP3, FLAC, Vorbis or Opus".to_string();
    let params = track.codec_params.as_ref().and_then(|params| params.audio()).ok_or_else(not_ours)?;
    // Gapless (the default): an MP3's encoder delay and padding are cut, so a loop loops
    let mut decoder = codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).map_err(|_| not_ours())?;

    let (mut left, mut right) = (Vec::new(), Vec::new());
    let (mut rate, mut stereo, mut truncated) = (0, false, false);
    let mut interleaved = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let audio = match decoder.decode(&packet) {
            Ok(audio) => audio,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let channels = audio.spec().channels().count().max(1);
        rate = audio.spec().rate();
        stereo = channels > 1;
        audio.copy_to_vec_interleaved(&mut interleaved);
        for frame in interleaved.chunks_exact(channels) {
            left.push(frame[0]);
            if stereo {
                right.push(frame[1]);
            }
        }
        let keep = (MAX_SAMPLE_SECONDS * rate.max(1) as f32) as usize;
        if left.len() > keep {
            left.truncate(keep);
            right.truncate(keep);
            truncated = true;
            break;
        }
    }
    if rate == 0 {
        return Err("no audio in the file".to_string());
    }
    if let Some(length) = length {
        left.truncate(length);
        right.truncate(length);
    }
    let sample = if stereo { SampleData::stereo(left, right, rate as f32) } else { SampleData::mono(left, rate as f32) };
    Ok(Decoded { sample, truncated })
}

/// symphonia's decoders, and ours for Opus.
fn codecs() -> &'static symphonia::core::codecs::registry::CodecRegistry {
    static CODECS: std::sync::OnceLock<symphonia::core::codecs::registry::CodecRegistry> = std::sync::OnceLock::new();
    CODECS.get_or_init(|| {
        let mut codecs = symphonia::core::codecs::registry::CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut codecs);
        codecs.register_audio_decoder::<super::opus::OpusDecoder>();
        codecs
    })
}

/// Reads a WAV, MP3, FLAC or Ogg file from disk.
pub fn read_file(path: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    decode(file, &path.to_string_lossy())
}

/// The bytes of a sample that ships with the examples, by key. In the
/// browser, [`LOADING`] until it's been fetched (asking starts the fetch).
fn shipped(key: &str) -> Result<&'static [u8], String> {
    let name = key.strip_prefix(EXAMPLE_PREFIX).ok_or("not an example sample")?;
    let shipped = EXAMPLE_SAMPLES.iter().find(|(path, _)| *path == name).ok_or("no such example sample")?;
    #[cfg(target_arch = "wasm32")]
    return super::example_fetch::fetch(shipped.0);
    #[cfg(not(target_arch = "wasm32"))]
    Ok(shipped.1)
}

/// The bytes of a sample that ships with the examples, by key, if they're
/// here (in the browser, once fetched).
pub fn example_bytes(key: &str) -> Option<&'static [u8]> {
    shipped(key).ok()
}

/// Loads the recording a key names: a shipped example's, or a file's.
pub fn load(key: &str) -> Result<Decoded, String> {
    if key.starts_with(EXAMPLE_PREFIX) {
        return decode(std::io::Cursor::new(shipped(key)?), key);
    }
    // Kept in the browser's IndexedDB, once the tab's own copy is gone
    #[cfg(target_arch = "wasm32")]
    if key.starts_with(DROPPED_PREFIX) {
        return decode(std::io::Cursor::new(super::tab_store::read(key)?), key);
    }
    let path = Path::new(key);
    if !path.exists() {
        return Err("file not found".to_string());
    }
    read_file(path)
}

/// Where a patch's relative sample paths start from.
#[derive(Clone, Copy, Debug)]
pub enum SampleBase<'a> {
    /// The folder the patch file is in.
    Folder(&'a Path),
    /// One of the examples, whose samples ship inside the app.
    Example,
    /// Nowhere: a patch from the clipboard or an autosave, whose sample
    /// paths are keys already.
    Keys,
}

/// The key for a sample path saved in a patch.
pub fn resolve(file: &str, base: SampleBase) -> String {
    if file.starts_with(EXAMPLE_PREFIX) || Path::new(file).is_absolute() {
        return file.to_string();
    }
    match base {
        SampleBase::Folder(folder) => {
            let relative: PathBuf = file.split('/').collect();
            folder.join(relative).to_string_lossy().into_owned()
        }
        SampleBase::Example => format!("{EXAMPLE_PREFIX}{file}"),
        SampleBase::Keys => file.to_string(),
    }
}

/// The path a patch saved in `folder` stores for the sample `key`:
/// relative, with forward slashes, for a file beside or below the patch,
/// and absolute otherwise. A shipped example's sample keeps its path among
/// the examples.
pub fn to_patch_path(key: &str, folder: &Path) -> String {
    if let Some(name) = key.strip_prefix(EXAMPLE_PREFIX) {
        return name.to_string();
    }
    match Path::new(key).strip_prefix(folder) {
        Ok(relative) => relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
        Err(_) => key.to_string(),
    }
}

/// Whether a sample would need copying to sit beside a patch in `folder`:
/// a file elsewhere, or a shipped example's.
pub fn is_outside(key: &str, folder: &Path) -> bool {
    key.starts_with(EXAMPLE_PREFIX) || Path::new(key).strip_prefix(folder).is_err()
}

/// The file name a key ends in, for showing on a node.
pub fn file_name(key: &str) -> &str {
    let name = key.strip_prefix(EXAMPLE_PREFIX).or_else(|| key.strip_prefix(DROPPED_PREFIX)).unwrap_or(key);
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Names each file the patch plays that's kept only in the browser's tab
/// as one in `folder` beside the patch (`Song samples`), so a patch
/// downloaded with its takes in that folder opens whole on the desktop.
/// Returns the files' names alone.
pub fn name_tab_files_in(patch: &mut super::Patch, folder: &str) -> Vec<String> {
    let mut names: Vec<String> = patch
        .all_nodes_mut()
        .into_iter()
        .filter_map(|node| {
            let name = node.file.as_deref()?.strip_prefix(DROPPED_PREFIX)?.to_string();
            node.file = Some(format!("{folder}/{name}"));
            Some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// In the browser, which has no folder beside a patch: names each file a
/// patch opened there names by a path relative to it (as a downloaded one
/// names its takes, in `Song samples/`) by the key the tab keeps a file of
/// that name under, so the takes kept from earlier come back with it.
/// Undoes [`name_tab_files_in`].
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn name_tab_files_within(patch: &mut super::Patch) {
    for node in patch.all_nodes_mut() {
        let Some(file) = node.file.as_mut() else { continue };
        if !file.starts_with(EXAMPLE_PREFIX) && !file.starts_with(DROPPED_PREFIX) && !Path::new(file.as_str()).is_absolute() {
            *file = format!("{DROPPED_PREFIX}{}", file_name(file));
        }
    }
}

/// The folder a patch saved as `patch` keeps its copied samples in:
/// `<patch name> samples`, beside it.
pub fn samples_folder(patch: &Path) -> PathBuf {
    let stem = patch.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "patch".to_string());
    patch.with_file_name(format!("{stem} samples"))
}

/// The folder a patch saved as `patch` keeps its Loopers' loops in:
/// `<patch name> loops`, beside it.
pub fn loops_folder(patch: &Path) -> PathBuf {
    let stem = patch.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "patch".to_string());
    patch.with_file_name(format!("{stem} loops"))
}

/// Writes 32-bit float WAV bytes for a recording, so nothing is lost: a
/// Looper's loop, saved, and tests and tools.
pub fn encode_wav(sample: &SampleData) -> Vec<u8> {
    let channels = if sample.is_stereo() { 2 } else { 1 };
    let spec = hound::WavSpec {
        channels,
        sample_rate: sample.sample_rate() as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut bytes, spec).expect("WAV header");
        for (i, &left) in sample.left().iter().enumerate() {
            writer.write_sample(left).expect("in memory");
            if channels == 2 {
                writer.write_sample(sample.right()[i]).expect("in memory");
            }
        }
        writer.finalize().expect("in memory");
    }
    bytes.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_it_encodes() {
        let stereo = SampleData::stereo(vec![0.5, -0.25, 0.0], vec![0.1, 0.2, 0.3], 44100.0);
        let decoded = decode_wav(std::io::Cursor::new(encode_wav(&stereo))).unwrap();
        assert_eq!(decoded.sample, stereo);
        assert!(!decoded.truncated);
        let mono = SampleData::mono(vec![0.5; 10], 22050.0);
        assert_eq!(decode_wav(std::io::Cursor::new(encode_wav(&mono))).unwrap().sample, mono);
    }

    #[test]
    fn decodes_integer_wavs() {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
            for s in [16384i16, -32768, 0] {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        let decoded = decode_wav(std::io::Cursor::new(bytes.into_inner())).unwrap();
        assert_eq!(decoded.sample.left(), &[0.5, -1.0, 0.0]);
    }

    #[test]
    fn a_float_wav_holding_nan_or_infinity_decodes_finite() {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut bytes, spec).unwrap();
            for s in [0.5f32, f32::NAN, f32::INFINITY, -1e30, 2.0] {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        let decoded = decode_wav(std::io::Cursor::new(bytes.into_inner())).unwrap();
        // Past full scale is kept, up to the ceiling; NaN and infinity are silence
        let ceiling = crate::dsp::finite::SAMPLE_CEILING;
        assert_eq!(decoded.sample.left(), &[0.5, 0.0, 0.0, -ceiling, 2.0]);
    }

    /// Peak level and frame count of a fixture, read as a Sampler reads it.
    fn fixture(name: &str) -> SampleData {
        read_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap().sample
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn decodes_mp3s_without_their_encoder_gap() {
        // Half a second of 440 Hz at 44.1 kHz, stereo, from ffmpeg's LAME
        let sample = fixture("sine-440-stereo.mp3");
        assert!(sample.is_stereo());
        assert_eq!(sample.sample_rate(), 44100.0);
        assert_eq!(sample.frames(), 22050);
        assert!((peak(sample.left()) - 0.084).abs() < 0.01, "{}", peak(sample.left()));
        // It starts at once, as the sine did: no encoder delay before it
        assert!(peak(&sample.left()[..64]) > 0.02);
    }

    #[test]
    fn decodes_ogg_vorbis() {
        // Half a second of 440 Hz at 48 kHz, mono
        let sample = fixture("sine-440-mono.ogg");
        assert!(!sample.is_stereo());
        assert_eq!(sample.sample_rate(), 48000.0);
        assert_eq!(sample.frames(), 24000);
        assert!(peak(&sample.left()[..64]) > 0.02);
        assert!((peak(sample.left()) - 0.125).abs() < 0.01, "{}", peak(sample.left()));
    }

    #[test]
    fn decodes_flac_exactly() {
        // Half a second of 440 Hz at 44.1 kHz, mono, lossless
        let sample = fixture("sine-440-mono.flac");
        assert!(!sample.is_stereo());
        assert_eq!(sample.sample_rate(), 44100.0);
        assert_eq!(sample.frames(), 22050);
        // ffmpeg's sine, to 16 bits: an eighth of full scale from the first sample
        for (i, s) in sample.left().iter().enumerate() {
            let sine = 0.125 * (std::f32::consts::TAU * 440.0 * i as f32 / 44100.0).sin();
            assert!((s - sine).abs() < 1e-3, "frame {i}: {s} for {sine}");
        }
    }

    /// How far `samples` lags a 440 Hz sine at `rate`, in frames, by the
    /// lag that best lines them up.
    fn lag_behind_sine(samples: &[f32], rate: f32) -> i32 {
        let sine = |i: i32| (std::f32::consts::TAU * 440.0 * i as f32 / rate).sin();
        let fit = |lag: i32| samples[400..4000].iter().enumerate().map(|(i, s)| s * sine(i as i32 + 400 - lag)).sum::<f32>();
        (-60..60).max_by(|a, b| fit(*a).total_cmp(&fit(*b))).unwrap()
    }

    #[test]
    fn decodes_ogg_opus_without_its_pre_skip() {
        // Half a second of 440 Hz at 48 kHz, stereo (so 3 dB down), from
        // libopus. Its encoder lays 312 frames of pre-skip before the sine
        let sample = fixture("sine-440-stereo.opus");
        assert!(sample.is_stereo());
        assert_eq!(sample.sample_rate(), 48000.0);
        assert_eq!(sample.frames(), 24000);
        assert!((peak(sample.left()) - 0.088).abs() < 0.01, "{}", peak(sample.left()));
        assert!((peak(sample.right()) - 0.088).abs() < 0.01, "{}", peak(sample.right()));
        // It starts at once, in phase with the sine it was
        assert!(peak(&sample.left()[..64]) > 0.02);
        assert_eq!(lag_behind_sine(sample.left(), 48000.0), 0);
    }

    #[test]
    fn decodes_ogg_opus_from_any_rate_at_48_khz() {
        // Mono at 44.1 kHz going in; Opus gives it back at 48
        let sample = fixture("sine-440-mono-44k.opus");
        assert!(!sample.is_stereo());
        assert_eq!(sample.sample_rate(), 48000.0);
        assert_eq!(sample.frames(), 24000);
        assert!((peak(sample.left()) - 0.125).abs() < 0.01, "{}", peak(sample.left()));
    }

    #[test]
    fn says_what_it_plays() {
        assert!(is_sound_file("kick.WAV") && is_sound_file("C:/loops/break.mp3") && is_sound_file("pad.ogg"));
        assert!(is_sound_file("take.flac") && is_sound_file("memo.opus"));
        assert!(!is_sound_file("notes.txt") && !is_sound_file("mp3"));
        let problem = decode(std::io::Cursor::new(b"not audio at all".to_vec()), "notes.txt").unwrap_err();
        assert_eq!(problem, "not a WAV, MP3, FLAC or Ogg file");
    }

    #[test]
    fn a_long_file_is_cut_to_the_cap() {
        // At 10 Hz, the cap is 3000 frames
        let long = SampleData::mono(vec![0.1; 3500], 10.0);
        let decoded = decode_wav(std::io::Cursor::new(encode_wav(&long))).unwrap();
        assert!(decoded.truncated);
        assert_eq!(decoded.sample.frames(), (MAX_SAMPLE_SECONDS * 10.0) as usize);
    }

    #[test]
    fn paths_are_relative_beside_or_below_the_patch_and_absolute_elsewhere() {
        let folder = std::env::temp_dir().join("patches");
        let below = folder.join("kit").join("kick.wav");
        assert_eq!(to_patch_path(&below.to_string_lossy(), &folder), "kit/kick.wav");
        assert_eq!(resolve("kit/kick.wav", SampleBase::Folder(&folder)), below.to_string_lossy());
        let elsewhere = std::env::temp_dir().join("elsewhere.wav");
        let key = elsewhere.to_string_lossy();
        assert_eq!(to_patch_path(&key, &folder), key);
        assert_eq!(resolve(&key, SampleBase::Folder(&folder)), key);
        assert!(is_outside(&key, &folder));
        assert!(!is_outside(&below.to_string_lossy(), &folder));
    }

    #[test]
    fn example_samples_resolve_to_keys_and_save_as_their_paths() {
        assert_eq!(resolve("samples/keys.wav", SampleBase::Example), "example:samples/keys.wav");
        assert_eq!(to_patch_path("example:samples/keys.wav", Path::new("anywhere")), "samples/keys.wav");
        assert!(is_outside("example:samples/keys.wav", Path::new("anywhere")));
        assert_eq!(file_name("example:samples/keys.wav"), "keys.wav");
        assert_eq!(file_name(r"C:\Music\choir ah.wav"), "choir ah.wav");
    }

    #[test]
    fn every_example_sample_loads() {
        for (path, _) in EXAMPLE_SAMPLES {
            let key = resolve(path, SampleBase::Example);
            let decoded = load(&key).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!decoded.sample.is_empty());
        }
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        assert!(load(&std::env::temp_dir().join("no-such-sample.wav").to_string_lossy()).is_err());
        assert!(load("example:samples/no-such.wav").is_err());
    }

    #[test]
    fn copied_samples_go_beside_the_patch() {
        let folder = samples_folder(Path::new("songs").join("Night Drive.json").as_path());
        assert_eq!(folder, Path::new("songs").join("Night Drive samples"));
        let loops = loops_folder(Path::new("songs").join("Night Drive.json").as_path());
        assert_eq!(loops, Path::new("songs").join("Night Drive loops"));
    }

    #[test]
    fn a_file_kept_in_the_tab_shows_its_name_alone() {
        assert_eq!(file_name("dropped:Take 1.wav"), "Take 1.wav");
        assert_eq!(file_name("example:samples/break.wav"), "break.wav");
        assert_eq!(file_name("songs/voice.wav"), "voice.wav");
    }

    #[test]
    fn a_downloaded_patch_names_its_tab_files_in_its_samples_folder() {
        use crate::persistence::{GroupData, NodeData, Patch};
        let sampler = |id, file: &str| {
            let mut node = NodeData::new(id, "sampler", (0.0, 0.0));
            node.file = Some(file.to_string());
            node
        };
        let mut patch = Patch::new("Song");
        patch.nodes.push(sampler(1, "dropped:Take 1.wav"));
        patch.nodes.push(sampler(2, "example:samples/break.wav"));
        patch.nodes.push(sampler(3, "dropped:Take 1.wav"));
        patch.groups.push(GroupData {
            id: 1,
            name: "Voices".into(),
            position: (0.0, 0.0),
            inputs: Vec::new(),
            outputs: Vec::new(),
            inputs_position: (0.0, 0.0),
            outputs_position: (0.0, 0.0),
            nodes: vec![sampler(4, "dropped:voice trimmed.wav")],
            connections: Vec::new(),
            groups: Vec::new(),
        });

        assert_eq!(name_tab_files_in(&mut patch, "Song samples"), ["Take 1.wav", "voice trimmed.wav"]);
        let files: Vec<_> = patch.all_nodes().iter().map(|n| n.file.clone().unwrap()).collect();
        assert_eq!(
            files,
            ["Song samples/Take 1.wav", "example:samples/break.wav", "Song samples/Take 1.wav", "Song samples/voice trimmed.wav"]
        );
        // Unzipped into a folder on the desktop, they're found in the folder beside it
        assert_eq!(
            resolve(&files[0], SampleBase::Folder(Path::new("Downloads"))),
            Path::new("Downloads").join("Song samples").join("Take 1.wav").to_string_lossy()
        );

        // Opened in the browser again, they're the takes the tab kept
        name_tab_files_within(&mut patch);
        let files: Vec<_> = patch.all_nodes().iter().map(|n| n.file.clone().unwrap()).collect();
        assert_eq!(files, ["dropped:Take 1.wav", "example:samples/break.wav", "dropped:Take 1.wav", "dropped:voice trimmed.wav"]);
        name_tab_files_within(&mut patch);
        assert_eq!(patch.all_nodes()[0].file.as_deref(), Some("dropped:Take 1.wav"), "named once");
    }
}
