//! Render a patch to a WAV file without opening the app.
//!
//! ```text
//! cargo run --release --bin render -- <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav] [--bench [--runs N]]
//! ```
//!
//! Prints the peak, true peak (ITU-R BS.1770-4) and RMS level of each
//! channel, and the integrated loudness (BS.1770-4) and loudness range (EBU
//! Tech 3342) of the mix, so renders can be compared before and after a
//! change. Then how hard the patch drove
//! the Audio Output's limiter: the peak going in, the most it turned the
//! output down, and for how much of the render. `--compare` reads an earlier
//! render and prints how far each of those has moved since.
//!
//! `--bench` times the render: processing only, not loading the patch or
//! measuring the result. It renders `--runs` times (3 unless given) and
//! reports the fastest, since a busy machine only ever slows a run down: the
//! realtime factor, and the nodes that took longest, in nanoseconds per
//! sample. `tools/nightly.py` reads its `bench:` line.
//!
//! Patches that need live input (Note In) render
//! silence unless something in the patch triggers them, such as a Clock or
//! Sequencer, or `--audition` is given to play a short phrase into them.
//! Audio Input modules are silent too, unless `--input` gives them a WAV file
//! to hear in place of the input device (resampled, if it was recorded at
//! another rate than the render's).
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
use soba::dsp::loudness::{loudness, true_peak, Loudness};
use soba::engine::{create_module_registry, Bench, NodeId, StereoBuffer, read_wav, read_wav_at, EngineCommand, LimiterReport, OfflineRenderer, BLOCK_SIZE};
use soba::persistence::sample_files::SampleBase;
use soba::persistence::{load_from_file, CompiledPatch, Patch};

const USAGE: &str =
    "usage: render <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--block-size N] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav] [--bench [--runs N]]";

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
    /// Time the render, best of `runs`.
    bench: bool,
    runs: usize,
}

/// How many of the heaviest nodes `--bench` lists.
const BENCH_NODES: usize = 5;

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut seconds = 5.0;
    let mut sample_rate = 48_000;
    let mut block_size = BLOCK_SIZE;
    let mut audition = false;
    let mut input = None;
    let mut cue = None;
    let mut compare = None;
    let mut bench = false;
    let mut runs = None;

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
            "--bench" => bench = true,
            "--runs" => runs = Some(value("--runs")?.parse().map_err(|e| format!("--runs: {}", e))?),
            "-h" | "--help" => return Err(USAGE.to_string()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {}\n{}", flag, USAGE)),
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    match <[PathBuf; 2]>::try_from(positional) {
        Ok([patch, out]) if seconds > 0.0 && block_size > 0 && runs != Some(0) => Ok(Args {
            patch,
            out,
            seconds,
            sample_rate,
            block_size,
            audition,
            input,
            cue,
            compare,
            bench,
            runs: runs.unwrap_or(if bench { 3 } else { 1 }),
        }),
        _ => Err(USAGE.to_string()),
    }
}

fn run(args: Args) -> Result<(), String> {
    let patch = load_from_file(&args.patch).map_err(|e| format!("{}: {}", args.patch.display(), e))?;
    let cues = match &args.cue {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?),
        None => None,
    };
    if cues.is_some() && args.audition {
        return Err("--cue and --audition can't be used together".to_string());
    }

    // Every run renders the same: the fastest is the one least slowed down
    let mut fastest: Option<Bench> = None;
    let mut last = None;
    for run in 0..args.runs {
        let (renderer, compiled, audio) = render(&args, &patch, cues.as_deref(), run == 0)?;
        let bench = renderer.bench();
        if fastest.as_ref().zip(bench.as_ref()).is_none_or(|(best, now)| now.process < best.process) {
            fastest = bench;
        }
        last = Some((renderer, compiled, audio));
    }
    let Some((renderer, compiled, audio)) = last else {
        return Err(USAGE.to_string());
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
    print_limiter(&renderer.limiter());
    // An engine node as the patch knows it: its module and its id in the file
    let describe = |engine: NodeId| {
        let node = compiled.node_ids.iter().find(|&(_, &e)| e == engine).map(|(&id, _)| id);
        let module = node.and_then(|id| patch.all_nodes().into_iter().find(|n| n.id == id)).map(|n| n.module_id.clone());
        format!(
            "{} (node {})",
            module.as_deref().unwrap_or("a module"),
            node.map_or("?".to_string(), |id| id.to_string())
        )
    };
    for &fault in renderer.faults() {
        println!("fault: {} made a NaN or infinity and was reset", describe(fault));
    }
    if let Some(bench) = &fastest {
        print_bench(bench, args.sample_rate, args.runs, describe);
    }
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

/// Builds the patch and renders it once. Warnings are printed on the
/// first run only, since every run gives the same ones.
fn render(
    args: &Args,
    patch: &Patch,
    cues: Option<&str>,
    first: bool,
) -> Result<(OfflineRenderer, CompiledPatch, StereoBuffer), String> {
    let warn = |message: String| {
        if first {
            eprintln!("{}", message);
        }
    };
    let (mut renderer, compiled) =
        OfflineRenderer::from_patch(patch, args.sample_rate as f32, args.block_size)
            .map_err(|e| e.to_string())?;
    for warning in &compiled.warnings {
        warn(format!("warning: {}", warning));
    }
    // Samples saved relative to the patch are beside it
    let folder = args.patch.parent().unwrap_or(std::path::Path::new("."));
    for warning in renderer.load_samples(&compiled, SampleBase::Folder(folder)) {
        warn(format!("warning: {}", warning));
    }
    if let Some(path) = &args.input {
        let (input, rate) = read_wav_at(path, args.sample_rate).map_err(|e| format!("{}: {}", path.display(), e))?;
        if rate != args.sample_rate {
            warn(format!("note: {} is {} Hz, resampled to {} Hz", path.display(), rate, args.sample_rate));
        }
        renderer.set_audio_input(input);
    }

    if args.bench {
        renderer.time();
    }
    let audio = if let Some(text) = cues {
        let commands = parse_cues(text, patch, &compiled, args.sample_rate as f32)?;
        renderer.render_with_commands(args.seconds, commands)
    } else if args.audition {
        renderer.render_audition(patch, args.seconds)
    } else {
        renderer.render_seconds(args.seconds)
    };
    Ok((renderer, compiled, audio))
}

/// What a render's levels are measured as.
struct Levels {
    /// Peak and RMS in dBFS and true peak in dBTP, left then right.
    peak: [f32; 2],
    true_peak: [f32; 2],
    rms: [f32; 2],
    loudness: Loudness,
}

impl Levels {
    fn of(left: &[f32], right: &[f32], sample_rate: u32) -> Self {
        Self {
            peak: [left, right].map(|c| amp_to_db(peak(c))),
            true_peak: [left, right].map(|c| amp_to_db(true_peak(c, sample_rate as f32))),
            rms: [left, right].map(|c| amp_to_db(rms(c))),
            loudness: loudness(left, right, sample_rate as f32),
        }
    }

    fn print(&self) {
        for (i, name) in ["L", "R"].iter().enumerate() {
            println!(
                "  {}: peak {:6.1} dBFS   true {:6.1} dBTP   rms {:6.1} dBFS",
                name, self.peak[i], self.true_peak[i], self.rms[i]
            );
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
                "  {}: peak {} dB     true {} dB     rms {} dB",
                name,
                change(self.peak[i], before.peak[i]),
                change(self.true_peak[i], before.true_peak[i]),
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

/// How hard the patch drove the output limiter, if it has one.
fn print_limiter(report: &LimiterReport) {
    if report.blocks == 0 {
        return;
    }
    println!(
        "  limiter in {:6.1} dBFS   took {:.1} dB at most, {:.0}% of the time over {} dB",
        amp_to_db(report.peak_in),
        // 0 - x rather than -x, so an idle limiter doesn't read -0.0
        0.0 - amp_to_db(report.deepest),
        report.time_limiting() * 100.0,
        LimiterReport::LIMITING_DB
    );
}

/// The fastest run's timing: the whole render, then the heaviest nodes.
fn print_bench(bench: &Bench, sample_rate: u32, runs: usize, describe: impl Fn(NodeId) -> String) {
    let frames = bench.frames.max(1);
    let audio = frames as f64 / sample_rate as f64;
    let process = bench.process.as_secs_f64();
    // nightly.py reads this line: keep its shape
    println!(
        "bench: {:.4} s for {:.2} s of audio, {:.1}x realtime (best of {})",
        process,
        audio,
        audio / process.max(f64::MIN_POSITIVE),
        runs
    );
    let total: u64 = bench.nodes().iter().map(|&(_, ns)| ns).sum();
    for (node, ns) in bench.nodes().into_iter().take(BENCH_NODES) {
        println!(
            "  {:<32} {:>7.1} ns/sample  {:>3.0}%",
            describe(node),
            ns as f64 / frames as f64,
            ns as f64 / total.max(1) as f64 * 100.0
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
/// param <module>[#n|@id] <parameter name> <value>`.
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
            return Err(err("expected <seconds> param <module>[#n|@id] <parameter> <value>"));
        }
        // `kind#n` is the nth of its kind, `kind@id` the one with that id in the file
        let (module, which) = match (words[2].split_once('@'), words[2].split_once('#')) {
            (Some((m, id)), _) => (m, Err(id.parse::<u64>().map_err(|_| err("bad @id"))?)),
            (None, Some((m, n))) => (m, Ok(n.parse::<usize>().map_err(|_| err("bad #n"))?.max(1) - 1)),
            (None, None) => (words[2], Ok(0)),
        };
        let value: f32 = words[words.len() - 1].parse().map_err(|_| err("not a value"))?;
        let name = words[3..words.len() - 1].join(" ");
        let mut kind = patch.all_nodes().into_iter().filter(|n| n.module_id == module);
        let found = match which {
            Ok(nth) => kind.nth(nth),
            Err(id) => kind.find(|n| n.id == id),
        };
        let node = found
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
