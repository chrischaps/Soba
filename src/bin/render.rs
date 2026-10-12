//! Render a patch to a WAV file without opening the app.
//!
//! ```text
//! cargo run --release --bin render -- <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav]
//! ```
//!
//! Prints the peak and RMS level of each channel, and the integrated loudness
//! (ITU-R BS.1770-4) and loudness range (EBU Tech 3342) of the mix, so renders
//! can be compared before and after a change. `--compare` reads an earlier
//! render and prints how far each of those has moved since.
//!
//! Patches that need live input (Keyboard, MIDI Note, Poly MIDI) render
//! silence unless something in the patch triggers them, such as a Clock or
//! Sequencer, or `--audition` is given to play a short phrase into them.
//! Audio Input modules are silent too, unless `--input` gives them a WAV file
//! to hear in place of the input device.
//!
//! `--cue` plays a script of parameter changes into the patch, in the
//! capture kit's `param` form, one per line (`#` starts a comment):
//!
//! ```text
//! 1.0   param util.looper Pedal Rec 1     # press the Looper's Rec
//! 1.05  param util.looper Pedal Rec 0     # and let go
//! 2.0   param filter.svf#2 Cutoff 800     # the second SVF Filter
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use soba::dsp::analysis::{amp_to_db, peak, rms};
use soba::dsp::loudness::{loudness, Loudness};
use soba::engine::{create_module_registry, read_wav, EngineCommand, OfflineRenderer};
use soba::persistence::sample_files::SampleBase;
use soba::persistence::{load_from_file, CompiledPatch, Patch};

const USAGE: &str =
    "usage: render <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--block-size N] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav]";

struct Args {
    patch: PathBuf,
    out: PathBuf,
    seconds: f32,
    sample_rate: u32,
    block_size: usize,
    audition: bool,
    /// A WAV file for Audio Input modules to hear.
    input: Option<PathBuf>,
    /// A script of parameter changes to play into the patch.
    cue: Option<PathBuf>,
    /// An earlier render to measure this one against.
    compare: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut seconds = 5.0;
    let mut sample_rate = 48_000;
    let mut block_size = 256;
    let mut audition = false;
    let mut input = None;
    let mut cue = None;
    let mut compare = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{} needs a value", name));
        match arg.as_str() {
            "--seconds" => seconds = value("--seconds")?.parse().map_err(|e| format!("--seconds: {}", e))?,
            "--sample-rate" => {
                sample_rate = value("--sample-rate")?.parse().map_err(|e| format!("--sample-rate: {}", e))?
            }
            "--block-size" => {
                block_size = value("--block-size")?.parse().map_err(|e| format!("--block-size: {}", e))?
            }
            "--audition" => audition = true,
            "--input" => input = Some(PathBuf::from(value("--input")?)),
            "--cue" => cue = Some(PathBuf::from(value("--cue")?)),
            "--compare" => compare = Some(PathBuf::from(value("--compare")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {}\n{}", flag, USAGE)),
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    match <[PathBuf; 2]>::try_from(positional) {
        Ok([patch, out]) if seconds > 0.0 && block_size > 0 => Ok(Args {
            patch,
            out,
            seconds,
            sample_rate,
            block_size,
            audition,
            input,
            cue,
            compare,
        }),
        _ => Err(USAGE.to_string()),
    }
}

fn run(args: Args) -> Result<(), String> {
    let patch = load_from_file(&args.patch).map_err(|e| format!("{}: {}", args.patch.display(), e))?;
    let (mut renderer, compiled) =
        OfflineRenderer::from_patch(&patch, args.sample_rate as f32, args.block_size)
            .map_err(|e| e.to_string())?;
    for warning in &compiled.warnings {
        eprintln!("warning: {}", warning);
    }
    // Samples saved relative to the patch are beside it
    let folder = args.patch.parent().unwrap_or(std::path::Path::new("."));
    for warning in renderer.load_samples(&compiled, SampleBase::Folder(folder)) {
        eprintln!("warning: {}", warning);
    }
    if let Some(path) = &args.input {
        let (input, rate) = read_wav(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        if rate != args.sample_rate {
            return Err(format!(
                "{} is {} Hz but the render is {} Hz: render with --sample-rate {}",
                path.display(),
                rate,
                args.sample_rate,
                rate
            ));
        }
        renderer.set_audio_input(input);
    }

    let audio = if let Some(path) = &args.cue {
        if args.audition {
            return Err("--cue and --audition can't be used together".to_string());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let commands = parse_cues(&text, &patch, &compiled, args.sample_rate as f32)?;
        renderer.render_with_commands(args.seconds, commands)
    } else if args.audition {
        renderer.render_audition(&patch, &compiled, args.seconds)
    } else {
        renderer.render_seconds(args.seconds)
    };

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: args.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&args.out, spec)
        .map_err(|e| format!("{}: {}", args.out.display(), e))?;
    for (&l, &r) in audio.left.iter().zip(&audio.right) {
        writer.write_sample(l).map_err(|e| e.to_string())?;
        writer.write_sample(r).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;

    println!(
        "rendered '{}' ({:.2} s @ {} Hz) -> {}",
        patch.name,
        args.seconds,
        args.sample_rate,
        args.out.display()
    );
    let levels = Levels::of(&audio.left, &audio.right, args.sample_rate);
    levels.print();
    if let Some(path) = &args.compare {
        let (before, rate) = read_wav(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        println!("against {}:", path.display());
        levels.print_change_from(&Levels::of(&before.left, &before.right, rate));
    }
    if audio.left.iter().chain(&audio.right).any(|s| !s.is_finite()) {
        return Err("render contains NaN or infinite samples".to_string());
    }
    Ok(())
}

/// What a render's levels are measured as.
struct Levels {
    /// Peak and RMS in dBFS, left then right.
    peak: [f32; 2],
    rms: [f32; 2],
    loudness: Loudness,
}

impl Levels {
    fn of(left: &[f32], right: &[f32], sample_rate: u32) -> Self {
        Self {
            peak: [left, right].map(|c| amp_to_db(peak(c))),
            rms: [left, right].map(|c| amp_to_db(rms(c))),
            loudness: loudness(left, right, sample_rate as f32),
        }
    }

    fn print(&self) {
        for (i, name) in ["L", "R"].iter().enumerate() {
            println!("  {}: peak {:6.1} dBFS   rms {:6.1} dBFS", name, self.peak[i], self.rms[i]);
        }
        println!(
            "  loudness {:>5} LUFS   range {:>4} LU",
            reading(self.loudness.integrated, false),
            reading(self.loudness.range, false)
        );
    }

    /// Prints how far each level has moved from `before`'s.
    fn print_change_from(&self, before: &Levels) {
        let change = |now: f32, then: f32| format!("{:+6.1}", now - then);
        for (i, name) in ["L", "R"].iter().enumerate() {
            println!(
                "  {}: peak {} dB     rms {} dB",
                name,
                change(self.peak[i], before.peak[i]),
                change(self.rms[i], before.rms[i])
            );
        }
        let change = |now: Option<f64>, then: Option<f64>| reading(now.zip(then).map(|(n, t)| n - t), true);
        println!(
            "  loudness {:>5} LU     range {:>4} LU",
            change(self.loudness.integrated, before.loudness.integrated),
            change(self.loudness.range, before.loudness.range)
        );
    }
}

/// A loudness reading to a tenth, `signed` for a change, or `--` where
/// there's nothing to read.
fn reading(value: Option<f64>, signed: bool) -> String {
    match value {
        Some(v) if signed => format!("{:+.1}", v),
        Some(v) => format!("{:.1}", v),
        None => "--".to_string(),
    }
}

/// Reads a cue script into engine commands at their frames: `<seconds>
/// param <module>[#n] <parameter name> <value>`.
fn parse_cues(text: &str, patch: &Patch, compiled: &CompiledPatch, sample_rate: f32) -> Result<Vec<(u64, EngineCommand)>, String> {
    let registry = create_module_registry();
    let mut commands = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let line = match line.find('#').filter(|&at| at == 0 || line[..at].ends_with(' ')) {
            Some(at) => &line[..at],
            None => line,
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let err = |msg: &str| format!("cue line {}: {} ({})", line_no + 1, msg, line.trim());
        let seconds: f64 = words[0].parse().map_err(|_| err("not a time"))?;
        if words.get(1) != Some(&"param") || words.len() < 5 {
            return Err(err("expected <seconds> param <module>[#n] <parameter> <value>"));
        }
        let (module, nth) = match words[2].split_once('#') {
            Some((m, n)) => (m, n.parse::<usize>().map_err(|_| err("bad #n"))?.max(1) - 1),
            None => (words[2], 0),
        };
        let value: f32 = words[words.len() - 1].parse().map_err(|_| err("not a value"))?;
        let name = words[3..words.len() - 1].join(" ");
        let node = patch
            .all_nodes()
            .into_iter()
            .filter(|n| n.module_id == module)
            .nth(nth)
            .and_then(|n| compiled.node_ids.get(&n.id).copied())
            .ok_or_else(|| err("no such module in the patch"))?;
        let param_index = registry
            .create(module)
            .and_then(|m| m.parameters().iter().position(|p| p.name == name))
            .ok_or_else(|| err("no such parameter"))?;
        let frame = (seconds * sample_rate as f64).round().max(0.0) as u64;
        commands.push((frame, EngineCommand::SetParameter { node_id: node, param_index, value }));
    }
    Ok(commands)
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{}", message);
            ExitCode::FAILURE
        }
    }
}
