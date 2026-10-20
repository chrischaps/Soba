//! Render a patch to a WAV file without opening the app.
//!
//! ```text
//! cargo run --release --bin render -- <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--block-size N] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav] [--bench [--runs N]] [--trace module:Port[,...] trace.csv [--trace-every S]] [--stems dir]
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
//!
//! `--trace` follows outputs as the render plays, named as the cue script
//! names modules, and writes them to a CSV: a row each `--trace-every`
//! seconds (each block unless given), the time at its end, then each
//! output's RMS (audio), highest (a gate, so no trigger is missed) or where
//! it ended (a CV). Which section of a song played when, say:
//!
//! ```text
//! --trace "seq.arranger:Section,seq.arranger:Bar,util.mixer#2:Out L" trace.csv --trace-every 0.5
//! ```
//!
//! `--stems` writes each Mixer channel that has something patched into it,
//! and each Mixer's return, to a WAV of its own in a folder, from the same
//! run: after the fader and the Master, before the Mixer's soft clip. A
//! Mixer's stems add up to its mix (a chained Mixer's to what it brings in).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use soba::dsp::analysis::{amp_to_db, peak, rms};
use soba::dsp::loudness::{loudness, true_peak, Loudness};
use soba::dsp::SignalType;
use soba::engine::{create_module_registry, Bench, GraphPlan, NodeId, StereoBuffer, read_wav, read_wav_at, EngineCommand, LimiterReport, OfflineRenderer, BLOCK_SIZE};
use soba::persistence::sample_files::SampleBase;
use soba::persistence::{load_from_file, CompiledPatch, NodeData, Patch};

const USAGE: &str =
    "usage: render <patch.json> <out.wav> [--seconds N] [--sample-rate HZ] [--block-size N] [--audition] [--input in.wav] [--cue cues.txt] [--compare before.wav] [--bench [--runs N]] [--trace module:Port[,...] trace.csv [--trace-every S]] [--stems dir]";

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
    /// Outputs to follow (`module:Port,...`) and the CSV to write them to.
    trace: Option<(String, PathBuf)>,
    /// How often a trace row is written, in seconds (each block if not given).
    trace_every: Option<f32>,
    /// A folder to write each Mixer channel's stem into.
    stems: Option<PathBuf>,
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
    let mut trace = None;
    let mut trace_every = None;
    let mut stems = None;

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
            "--trace" => trace = Some((value("--trace")?, PathBuf::from(value("--trace")?))),
            "--trace-every" => {
                trace_every = Some(value("--trace-every")?.parse().map_err(|e| format!("--trace-every: {}", e))?)
            }
            "--stems" => stems = Some(PathBuf::from(value("--stems")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {}\n{}", flag, USAGE)),
            _ => positional.push(PathBuf::from(arg)),
        }
    }

    match <[PathBuf; 2]>::try_from(positional) {
        Ok([patch, out]) if seconds > 0.0 && block_size > 0 && runs != Some(0) && trace_every.is_none_or(|s| s > 0.0) => Ok(Args {
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
            trace,
            trace_every,
            stems,
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

    // What --trace and --stems write, as the first run plays
    let taps = Arc::new(Mutex::new(Taps::default()));

    // Every run renders the same: the fastest is the one least slowed down
    let mut fastest: Option<Bench> = None;
    let mut last = None;
    for run in 0..args.runs {
        let taps = (run == 0).then_some(&taps);
        let (renderer, compiled, audio) = render(&args, &patch, cues.as_deref(), taps)?;
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
    taps.lock().map_err(|e| e.to_string())?.finish()?;
    if let Some((_, path)) = &args.trace {
        println!("trace -> {}", path.display());
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
/// first run only, since every run gives the same ones, and only the first
/// is given `taps` to write as it plays.
fn render(
    args: &Args,
    patch: &Patch,
    cues: Option<&str>,
    taps: Option<&Arc<Mutex<Taps>>>,
) -> Result<(OfflineRenderer, CompiledPatch, StereoBuffer), String> {
    let first = taps.is_some();
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

    if let Some(taps) = taps {
        let mut opened = taps.lock().map_err(|e| e.to_string())?;
        if let Some((specs, path)) = &args.trace {
            let every = args.trace_every.map_or(args.block_size, |s| (s * args.sample_rate as f32).round() as usize);
            opened.trace = Some(Trace::new(specs, path, every, args.sample_rate, patch, &compiled)?);
        }
        if let Some(dir) = &args.stems {
            opened.stems = Some(Stems::new(dir, args.sample_rate, patch, &compiled)?);
            renderer.keep_stems();
        }
        if args.trace.is_some() || args.stems.is_some() {
            let taps = Arc::clone(taps);
            renderer.watch(Box::new(move |plan, len| {
                if let Ok(mut taps) = taps.lock() {
                    taps.block(plan, len);
                }
            }));
        }
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
        let (found, node) = find_node(patch, compiled, words[2]).map_err(err)?;
        let value: f32 = words[words.len() - 1].parse().map_err(|_| err("not a value"))?;
        let name = words[3..words.len() - 1].join(" ");
        let param_index = registry
            .create(&found.module_id)
            .and_then(|m| m.parameters().iter().position(|p| p.name == name))
            .ok_or_else(|| err("no such parameter"))?;
        let frame = (seconds * sample_rate as f64).round().max(0.0) as u64;
        commands.push((frame, EngineCommand::SetParameter { node_id: node, param_index, value }));
    }
    Ok(commands)
}

/// A module as cue scripts and `--trace` name it: `kind` (the first of its
/// kind), `kind#n` (the nth) or `kind@id` (the one with that id in the file).
fn find_node<'p>(patch: &'p Patch, compiled: &CompiledPatch, word: &str) -> Result<(&'p NodeData, NodeId), &'static str> {
    let (module, which) = match (word.split_once('@'), word.split_once('#')) {
        (Some((m, id)), _) => (m, Err(id.parse::<u64>().map_err(|_| "bad @id")?)),
        (None, Some((m, n))) => (m, Ok(n.parse::<usize>().map_err(|_| "bad #n")?.max(1) - 1)),
        (None, None) => (word, Ok(0)),
    };
    let mut kind = patch.all_nodes().into_iter().filter(|n| n.module_id == module);
    let found = match which {
        Ok(nth) => kind.nth(nth),
        Err(id) => kind.find(|n| n.id == id),
    };
    found
        .and_then(|n| compiled.node_ids.get(&n.id).map(|&engine| (n, engine)))
        .ok_or("no such module in the patch")
}

/// One output `--trace` follows.
struct Traced {
    node: NodeId,
    /// Counted among the module's outputs.
    output: usize,
    signal: SignalType,
    /// Over the row so far: the sum of squares for audio, the highest for a
    /// gate, the latest for a CV.
    value: f64,
}

/// Writes the traced outputs to a CSV as the render plays, a row each
/// `every` frames (to the block): the time at its end, then for each output
/// its RMS (audio, its voices summed), its highest (a gate, in any voice) or
/// where it ended (a CV, its first voice).
struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    every: usize,
    traced: Vec<Traced>,
    /// Frames in the row so far, and in the whole render.
    frames: usize,
    total: u64,
    sample_rate: f64,
}

impl Trace {
    /// Reads `specs` (`module[#n|@id]:Port[,...]`) against the patch and
    /// starts the CSV with their names.
    fn new(specs: &str, path: &Path, every: usize, sample_rate: u32, patch: &Patch, compiled: &CompiledPatch) -> Result<Self, String> {
        let registry = create_module_registry();
        let mut traced = Vec::new();
        for spec in specs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let err = |msg: &str| format!("--trace {}: {}", spec, msg);
            let (word, port) = spec.split_once(':').ok_or_else(|| err("expected module[#n|@id]:Port"))?;
            let (node, engine) = find_node(patch, compiled, word).map_err(err)?;
            let module = registry.create(&node.module_id).ok_or_else(|| err("unknown module"))?;
            let outputs = module.ports().iter().filter(|p| p.is_output());
            let (output, definition) = outputs.enumerate().find(|(_, p)| p.name == port).ok_or_else(|| err("no such output"))?;
            if !matches!(definition.signal_type, SignalType::Audio | SignalType::Control | SignalType::Gate) {
                return Err(err("only audio, CV and gate outputs can be traced"));
            }
            traced.push((spec.to_string(), Traced { node: engine, output, signal: definition.signal_type, value: 0.0 }));
        }
        if traced.is_empty() {
            return Err("--trace needs at least one module:Port".to_string());
        }
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut out = std::io::BufWriter::new(file);
        // A name with a comma or quote in it is quoted, as CSV has it
        let names: Vec<String> = traced.iter().map(|(name, _)| format!("\"{}\"", name.replace('"', "\"\""))).collect();
        writeln!(out, "seconds,{}", names.join(",")).map_err(|e| e.to_string())?;
        let traced = traced.into_iter().map(|(_, t)| t).collect();
        Ok(Self { out, every: every.max(1), traced, frames: 0, total: 0, sample_rate: sample_rate as f64 })
    }

    /// Takes in the first `len` frames of the block just played.
    fn block(&mut self, plan: &GraphPlan, len: usize) -> std::io::Result<()> {
        for traced in &mut self.traced {
            let Some(buffer) = plan.output(traced.node, traced.output) else { continue };
            let len = len.min(buffer.samples.len());
            if len == 0 {
                continue;
            }
            let voices = 0..buffer.channels();
            match traced.signal {
                SignalType::Audio => {
                    for i in 0..len {
                        let sum: f32 = voices.clone().map(|c| buffer.voice(c).samples[i]).sum();
                        traced.value += sum as f64 * sum as f64;
                    }
                }
                SignalType::Gate => {
                    let high = voices.flat_map(|c| &buffer.voice(c).samples[..len]).fold(f32::MIN, |a, &b| a.max(b));
                    traced.value = traced.value.max(high as f64);
                }
                _ => traced.value = buffer.samples[len - 1] as f64,
            }
        }
        self.frames += len;
        self.total += len as u64;
        if self.frames >= self.every {
            self.row()?;
        }
        Ok(())
    }

    /// Writes the row so far, and starts the next.
    fn row(&mut self) -> std::io::Result<()> {
        if self.frames == 0 {
            return Ok(());
        }
        write!(self.out, "{:.4}", self.total as f64 / self.sample_rate)?;
        for traced in &mut self.traced {
            let value = match traced.signal {
                SignalType::Audio => (traced.value / self.frames as f64).sqrt(),
                _ => traced.value,
            };
            write!(self.out, ",{:.6}", value)?;
            if traced.signal != SignalType::Control {
                traced.value = 0.0;
            }
        }
        writeln!(self.out)?;
        self.frames = 0;
        Ok(())
    }

    /// Writes the last row, short or not, and closes the file.
    fn finish(&mut self) -> std::io::Result<()> {
        self.row()?;
        self.out.flush()
    }
}

/// A stem `--stems` writes: one part of a module's mix.
struct Stem {
    node: NodeId,
    name: &'static str,
    /// What it's written as, and where it comes from (`None` for a part
    /// nothing is patched into, which isn't written).
    written: Option<(PathBuf, String, hound::WavWriter<std::io::BufWriter<std::fs::File>>)>,
    peak: f32,
    squares: f64,
    frames: u64,
}

/// Writes each part a Mixer (or any module that keeps stems) mixes to a WAV
/// of its own as the render plays, after its fader and the Master.
struct Stems {
    dir: PathBuf,
    sample_rate: u32,
    stems: Vec<Stem>,
    /// For naming each stem: the module of every engine node, and the
    /// cables between them as the engine has them.
    modules: HashMap<NodeId, (String, u64)>,
    cables: Vec<(NodeId, usize, NodeId, usize)>,
}

impl Stems {
    fn new(dir: &Path, sample_rate: u32, patch: &Patch, compiled: &CompiledPatch) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let nodes = patch.all_nodes();
        let modules = compiled
            .node_ids
            .iter()
            .filter_map(|(&id, &engine)| nodes.iter().find(|n| n.id == id).map(|n| (engine, (n.module_id.clone(), id))))
            .collect();
        let cables = compiled
            .commands
            .iter()
            .filter_map(|c| match *c {
                EngineCommand::Connect { from_node, from_port, to_node, to_port } => Some((from_node, from_port, to_node, to_port)),
                _ => None,
            })
            .collect();
        Ok(Self { dir: dir.to_path_buf(), sample_rate, stems: Vec::new(), modules, cables })
    }

    /// What feeds the part `name` of `node`: a cable into its audio input
    /// `name`, or `name L` or `name R`, as `module@id Port`.
    fn source(modules: &HashMap<NodeId, (String, u64)>, cables: &[(NodeId, usize, NodeId, usize)], node: NodeId, name: &str) -> Option<String> {
        let registry = create_module_registry();
        let ports = |engine: NodeId| modules.get(&engine).and_then(|(module, _)| registry.create(module));
        let module = ports(node)?;
        let feeds = |port: usize| {
            module.ports().get(port).is_some_and(|p| {
                p.is_input() && p.signal_type == SignalType::Audio && [name.to_string(), format!("{name} L"), format!("{name} R")].contains(&p.name.to_string())
            })
        };
        let &(from, from_port, _, _) = cables.iter().find(|&&(_, _, to, port)| to == node && feeds(port))?;
        let (module, id) = modules.get(&from)?;
        let port = ports(from).and_then(|m| m.ports().get(from_port).map(|p| p.name)).unwrap_or("?");
        Some(format!("{module}@{id} {port}"))
    }

    fn block(&mut self, plan: &GraphPlan, len: usize) -> Result<(), String> {
        let Self { dir, sample_rate, stems, modules, cables } = self;
        let mut result = Ok(());
        plan.stems(|node, name, left, right| {
            if result.is_err() {
                return;
            }
            let index = match stems.iter().position(|s| (s.node, s.name) == (node, name)) {
                Some(index) => index,
                // Seen for the first time: open its file if something plays into it
                None => {
                    let written = Self::source(modules, cables, node, name).map(|source| {
                        let (module, id) = &modules[&node];
                        let file = format!("{module}@{id}-{}.wav", name.to_lowercase().replace(' ', ""));
                        let path = dir.join(file);
                        let spec = hound::WavSpec { channels: 2, sample_rate: *sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
                        hound::WavWriter::create(&path, spec).map(|writer| (path, source, writer))
                    });
                    let written = match written.transpose() {
                        Ok(written) => written,
                        Err(e) => {
                            result = Err(e.to_string());
                            return;
                        }
                    };
                    stems.push(Stem { node, name, written, peak: 0.0, squares: 0.0, frames: 0 });
                    stems.len() - 1
                }
            };
            let stem = &mut stems[index];
            let Some((_, _, writer)) = &mut stem.written else { return };
            for (&l, &r) in left[..len].iter().zip(&right[..len]) {
                if let Err(e) = writer.write_sample(l).and_then(|()| writer.write_sample(r)) {
                    result = Err(e.to_string());
                    return;
                }
                stem.peak = stem.peak.max(l.abs()).max(r.abs());
                stem.squares += (l as f64 * l as f64 + r as f64 * r as f64) / 2.0;
            }
            stem.frames += len as u64;
        });
        result
    }

    /// Closes every file, printing what each stem holds.
    fn finish(&mut self) -> Result<(), String> {
        println!("stems -> {}:", self.dir.display());
        for stem in self.stems.drain(..) {
            let Some((path, source, writer)) = stem.written else { continue };
            writer.finalize().map_err(|e| format!("{}: {}", path.display(), e))?;
            let (module, id) = &self.modules[&stem.node];
            let rms = (stem.squares / stem.frames.max(1) as f64).sqrt() as f32;
            let level = |amp: f32| if amp > 0.0 { format!("{:6.1} dBFS", amp_to_db(amp)) } else { format!("{:>11}", "silent") };
            println!(
                "  {:<24} <- {:<28} peak {}   rms {}   {}",
                format!("{module}@{id} {}", stem.name),
                source,
                level(stem.peak),
                level(rms),
                path.file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned())
            );
        }
        Ok(())
    }
}

/// What `--trace` and `--stems` write as the render plays, shown each block.
#[derive(Default)]
struct Taps {
    trace: Option<Trace>,
    stems: Option<Stems>,
    /// The first thing that went wrong writing either.
    error: Option<String>,
}

impl Taps {
    fn block(&mut self, plan: &GraphPlan, len: usize) {
        if self.error.is_some() {
            return;
        }
        let trace = self.trace.as_mut().map_or(Ok(()), |t| t.block(plan, len).map_err(|e| format!("--trace: {e}")));
        let stems = self.stems.as_mut().map_or(Ok(()), |s| s.block(plan, len).map_err(|e| format!("--stems: {e}")));
        self.error = trace.and(stems).err();
    }

    fn finish(&mut self) -> Result<(), String> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if let Some(trace) = &mut self.trace {
            trace.finish().map_err(|e| format!("--trace: {e}"))?;
        }
        if let Some(stems) = &mut self.stems {
            stems.finish()?;
        }
        Ok(())
    }
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
