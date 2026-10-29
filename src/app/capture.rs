//! Frame-stepped capture: films the app with its sound, in lockstep.
//!
//! `soba <patch.json> --capture <script.txt> --out <dir>` opens the
//! patch, plays a timed script into it, and records every frame along with
//! exactly the audio that frame covers. The clock advances one video frame
//! per UI frame, however long each frame takes to draw and save, and the
//! audio engine renders that frame's samples by hand instead of running on
//! the device. Picture and sound can't drift apart, and a script plays out
//! the same way every time. The engine cuts its blocks where `render` does,
//! counted from the first recorded frame, so a script that plays from 0
//! records what `render` makes of the patch, sample for sample. MIDI cues
//! land on their sample; a knob, key or `param` cue reaches the sound at
//! the next block edge.
//!
//! The script drives the app through its real input paths: key presses
//! reach the QWERTY keyboard, mouse moves and drags reach the node graph and
//! knobs, MIDI notes reach Note In modules at the exact sample they're
//! scheduled for. Real mouse and keyboard input is ignored
//! while capturing.
//!
//! Output, in `--out`:
//! - `video.mkv`: RGB frames (ffmpeg must be on the PATH), lossless unless
//!   `--crf N [--preset P]` says otherwise. Lossless is about 20 MB a second
//!   at 1080p30, so a shot of more than a minute or two wants `--crf 12
//!   --preset fast`: about a quarter the size for a busy patch, with nothing
//!   visible lost once it's encoded for the web.
//! - `audio.wav`: 32-bit float stereo
//! - `<name>.ppm`: stills taken with the `still` cue
//!
//! # Script
//!
//! One cue per line, `<seconds> <cue> <args...>`; `#` starts a comment.
//! Coordinates are in points (the window is `size / ppp` points across),
//! aimed at the `--ppp` the script is filmed at. The layout shifts a little
//! from one ppp to another, since text rows snap to whole pixels: by the
//! foot of a tall module, a button can sit 7 points lower at ppp 2 than at
//! ppp 1. So aim from a still filmed at the shot's own ppp (pixel / ppp), or
//! better, at a module's part by name, which is found wherever it was last
//! drawn. A press or right-click that just misses a control says so on
//! stderr.
//!
//! ```text
//! 0.0  play                         # start the transport: a capture
//!                                   # starts stopped, and says so at the
//!                                   # end if it never played
//! 0.5  key Z 0.4                    # tap a QWERTY key, held 0.4 s
//! 0.5  key Ctrl+Shift+F 0.1         # a shortcut, with its modifiers held
//! 0.5  type osc 0.1                 # type text, a letter every 0.1 s
//! 0.5  type "two words" 0.05        # quoted, for text with spaces
//! 1.0  note 60 100 2.0              # MIDI note, velocity, length
//! 1.0  note 36 110 0.1 10           # ...and its channel, 1-16 (1 if left out):
//!                                   # a pad on channel 10
//! 1.0  chord 60,64,67 90 3.0
//! 1.0  cc 1 74 127                  # a MIDI CC: channel, number, value
//! 1.0  bend 1 -8192                 # pitch bend: channel, -8192 to 8191
//! 1.0  pressure 1 90                # channel pressure: channel, 0-127
//! 1.0  midi start                   # MIDI Start, Stop or Continue, or one
//!                                   # clock tick (`midi clock`)
//! 1.0  midiclock 120 8.0            # MIDI clock ticks at 120 BPM for 8 s
//! 2.0  param osc.sine Detune 12 1.5 # module (#n for the nth, @id for its id
//!                                   # in the file), input, value, ramp
//! 2.0  param filter.svf#2 Cutoff 800
//! 3.0  cursor on                    # draw a pointer over the picture
//! 3.0  move 400 300 0.6             # glide the pointer
//! 3.0  move source.sampler file 0.3 # ...to a module's part, wherever it's
//!                                   # drawn: see capture_aim for the names
//! 3.0  move osc.sine#2 knob:Detune  # a knob's dial; in:CV, out:Out a jack
//! 3.0  move fx.eq 234 135 0.3       # patch points from a module's corner
//! 3.0  move seq.chord step:2 0.3    # a step, cell, pedal or section
//! 3.0  move frame:Shape 0.3         # a frame's title bar, or x y from its corner
//! 3.0  move by 0 -60 2.0            # from where the pointer is
//! 3.0  move off                     # take the pointer out of the window,
//!                                   # so nothing under it lights up
//! 3.5  drag 500 340 0.8             # press, glide, release (to any of
//!                                   # the same spots)
//! 3.5  press / release
//! 3.5  rclick                       # right-click, for context menus
//! 3.5  wheel -120                   # turn the mouse wheel (points, down < 0)
//! 3.5  wheel 240 ctrl               # ...with Ctrl held, which zooms
//! 3.5  wheel 120 alt                # ...with Alt, which turns a knob under it
//! 4.0  view 120 -40 2.0             # pan the graph to an offset over 2 s
//! 4.0  zoom 1.4 2.0                 # zoom the graph by a factor over 2 s
//! 4.0  fit                          # frame the whole level, as F does
//! 4.0  fit 1.5                      # ...gliding there over 1.5 s
//! 4.0  aim 1200 640 1.5 2.0         # glide to centre patch point (1200,
//!                                   # 640) at zoom 1.5 over 2 s, on the
//!                                   # level on show (a layout's patch space)
//! 4.0  look filter.svf 1.2          # frame a module, as F frames it
//!                                   # selected, over 1.2 s (cut, without)
//! 4.0  look osc.sine#2 1.4 2.0      # ...or centre it at zoom 1.4, over 2 s
//! 4.0  look close util.vca 1.2      # ...or frame it as close as it fits
//!                                   # (up to 5x): its knobs fill the shot
//! 4.0  look frame:Drive and echo 1  # ...or a frame, by its title
//! 4.0  enter Bells                 # dive into a group through its
//!                                   # miniature, as a double-click does
//! 4.0  enter Bells/FM Bell          # ...a path from the top, where several
//!                                   # groups share a name
//! 4.0  enter Strings/Pump#2         # ...the second of two side by side,
//!                                   # counting down the page, then across
//! 4.0  enter Drums at 168 291 1.0   # ...landing aimed at a patch point
//!                                   # inside, at a zoom, rather than on all
//!                                   # of it: the dive and the aim are one move
//! 4.0  leave                        # dive back out, as Escape does
//! 4.0  camera -300 0 0.5 3.0        # move a virtual camera: the view as
//!                                   # first framed (or last fitted, aimed
//!                                   # or dived into), scaled
//!                                   # 0.5 about the centre and shifted
//!                                   # (-300, 0), over 3 s
//! 5.0  still filter-opens           # save this frame as filter-opens.ppm
//! 5.0  layout voice                 # save where each module, frame and
//!                                   # note is drawn, in patch space, and
//!                                   # where the view is, as voice.json
//! 5.0  input voice.wav              # Audio Input modules hear this WAV
//!                                   # from now on, in place of a device,
//!                                   # resampled to the capture's rate
//! 6.0  hover take.wav               # drag a file over the window, from
//! 7.0  drop                         # the desktop, and drop it at the pointer
//! 7.0  hover off                    # ...or give the drag up
//! 8.0  land                         # whatever's moving (knobs, the camera,
//!                                   # the pointer) gets there now: a
//!                                   # presentation's take writes it where
//!                                   # a beat was finished early
//! 9.0  end
//! ```
//!
//! The cues are parsed in `cues` and played into the app by a
//! `CueRunner`, which a presentation drives too; this is the recorder
//! around them: the frame clock, the audio, the files.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Pos2, Rect, Vec2};

use super::capture_aim::{self, Part, Spot, Which};
pub use super::cue_runner::CaptureAction;
use super::cue_runner::CueRunner;
use super::cues::{parse_cues, Cue, Mode};
use crate::dsp::InputAudio;
use crate::engine::{read_input_at, ChannelBuffer, MidiEvent, BLOCK_SIZE};

/// The fewest frames drawn before recording starts, so the window size,
/// theme and patch have settled. Cues at negative times lengthen the warmup
/// to reach them: a shot can start the transport, set its view and let a
/// reverb fill before the first recorded frame. A long one costs little:
/// see `FAST_FORWARD_MARGIN`.
const WARMUP_FRAMES: i64 = 45;

/// A long warmup (a still three minutes into a song) runs its frames' cues
/// and audio without drawing them until it's this many seconds from the
/// first recorded frame. The app keeps cables' and meters' histories as it
/// goes; the drawn seconds after are for anything that only moves as it's
/// drawn, such as a face's animation, to catch up.
const FAST_FORWARD_MARGIN: f64 = 3.0;

/// Seconds of a fast-forward stepped between drawn frames, so the window
/// keeps answering the system while it runs.
const FAST_FORWARD_STRIDE: f64 = 5.0;

/// How near a press can come to a control without landing on it before the
/// capture says so, in points.
const NEAR_MISS: f32 = 6.0;

/// How the capture is set up, from the command line.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub script: PathBuf,
    pub out_dir: PathBuf,
    pub fps: u32,
    pub sample_rate: u32,
    /// Output size in pixels.
    pub size: [u32; 2],
    /// Pixels per point: how large the interface is drawn.
    pub ppp: f32,
    /// x264's quality for the video: 0 is lossless.
    pub crf: u32,
    /// x264's speed preset: slower presets make smaller files.
    pub preset: String,
}

impl CaptureConfig {
    /// Reads `--capture <script> [--out DIR] [--fps N] [--size WxH] [--ppp X]
    /// [--crf N] [--preset P]` from the command line, or `None` without `--capture`.
    pub fn from_args(args: &[String]) -> Result<Option<Self>, String> {
        let Some(at) = args.iter().position(|a| a == "--capture") else {
            return Ok(None);
        };
        let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
        let script = args.get(at + 1).ok_or("--capture needs a script")?;
        let size = match value("--size") {
            Some(s) => {
                let (w, h) = s.split_once('x').ok_or("--size is WxH")?;
                [w.parse().map_err(|_| "bad --size")?, h.parse().map_err(|_| "bad --size")?]
            }
            None => [1920, 1080],
        };
        Ok(Some(Self {
            script: PathBuf::from(script),
            out_dir: PathBuf::from(value("--out").unwrap_or_else(|| "capture".into())),
            fps: value("--fps").map(|f| f.parse()).transpose().map_err(|_| "bad --fps")?.unwrap_or(60),
            sample_rate: 48_000,
            size,
            ppp: value("--ppp").map(|f| f.parse()).transpose().map_err(|_| "bad --ppp")?.unwrap_or(1.5),
            crf: value("--crf").map(|f| f.parse()).transpose().map_err(|_| "bad --crf")?.unwrap_or(0),
            preset: value("--preset").unwrap_or_else(|| "ultrafast".into()),
        }))
    }

    /// The window's inner size in points.
    pub fn size_in_points(&self) -> Vec2 {
        Vec2::new(self.size[0] as f32, self.size[1] as f32) / self.ppp
    }

    /// The command-line arguments that aren't capture options, such as the
    /// patch to open.
    pub fn is_option(arg: &str) -> bool {
        matches!(arg, "--capture" | "--out" | "--fps" | "--size" | "--ppp" | "--crf" | "--preset")
    }
}

/// The running capture.
pub struct Capture {
    config: CaptureConfig,
    /// The frame being drawn; negative while warming up.
    frame: i64,
    /// How many frames the warmup lasts.
    warmup: i64,
    /// Whether this UI pass is a new frame (rather than a repaint while
    /// waiting for the last frame's screenshot).
    fresh: bool,
    /// Whether a UI pass has drawn the current frame.
    drawn: bool,
    /// The input the current frame began with, to give it again should the
    /// pass that began it draw nothing.
    frame_input: egui::RawInput,
    /// Frames stepped without drawing since the last drawn one.
    skipped: i64,
    /// A screenshot has been asked for and hasn't arrived.
    awaiting_shot: bool,
    /// Where the screenshot's callback leaves it, after the frame is painted.
    shot: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
    ffmpeg: Option<Child>,
    wav: Option<hound::WavWriter<BufWriter<File>>>,
    /// The audio rendered this frame, interleaved stereo.
    audio: Vec<f32>,
    /// Samples taken by frames so far, so each frame takes its exact share.
    samples_done: u64,
    /// Where the engine's blocks are counted from: the first recorded
    /// frame, as `render` counts them from its first sample.
    grid: i64,
    /// How far the engine has rendered, always on the grid. It runs up to a
    /// block ahead of the frames.
    rendered: i64,
    /// Audio rendered past the frames, for the next frame to take first.
    carry: Vec<f32>,
    /// The script's MIDI, kept apart from its other cues: it's sent as
    /// far ahead as the engine renders, so each event lands on its sample.
    midi: Vec<(f64, MidiEvent)>,
    next_midi: usize,
    stills: Vec<String>,
    /// Plays the script's cues into the app.
    runner: CueRunner,

    finished: bool,
    size_checked: bool,
    /// A WAV playing into Audio Input modules, with every channel it has,
    /// and the sample it started at.
    input: Option<(ChannelBuffer, i64)>,
    /// The share of `input` the engine renders this frame, interleaved.
    input_frame: Vec<f32>,
    /// Whether the transport ran during any recorded frame. A capture
    /// starts stopped, so a script that never plays it films silence.
    played: bool,
}

impl Capture {
    /// Reads the script and opens the outputs.
    pub fn start(config: CaptureConfig) -> Result<Self, String> {
        let text = std::fs::read_to_string(&config.script)
            .map_err(|e| format!("can't read {}: {}", config.script.display(), e))?;
        let mut capture = Self::from_script(config, &text)?;
        let config = &capture.config;
        crate::graph::marks::start();
        std::fs::create_dir_all(&config.out_dir).map_err(|e| e.to_string())?;

        let wav = hound::WavWriter::create(
            config.out_dir.join("audio.wav"),
            hound::WavSpec {
                channels: 2,
                sample_rate: config.sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|e| e.to_string())?;

        let [w, h] = config.size;
        let ffmpeg = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{}x{}", w, h), "-r", &config.fps.to_string(), "-i", "-"])
            .args(["-c:v", "libx264rgb", "-crf", &config.crf.to_string(), "-preset", &config.preset])
            .arg(config.out_dir.join("video.mkv"))
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("can't start ffmpeg: {}", e))?;

        capture.wav = Some(wav);
        capture.ffmpeg = Some(ffmpeg);
        Ok(capture)
    }

    /// A capture playing `text`, with nowhere to write yet.
    fn from_script(config: CaptureConfig, text: &str) -> Result<Self, String> {
        let cues = parse_cues(text, Mode::Film)?;
        let earliest = cues.first().map_or(0.0, |c| c.0).min(0.0);
        let warmup = WARMUP_FRAMES.max((-earliest * config.fps as f64).ceil() as i64 + 1);
        let grid = (warmup as f64 * config.sample_rate as f64 / config.fps as f64).round() as i64;
        let (midi, cues): (Vec<_>, Vec<_>) = cues.into_iter().partition(|(_, cue)| matches!(cue, Cue::Note { .. } | Cue::Midi(_)));
        let midi = midi.into_iter().filter_map(|(at, cue)| Some((at, cue.midi_event()?))).collect();
        for (at, cue) in &cues {
            if let Cue::Move { to: Spot::Part(part), .. } = cue {
                capture_aim::check(part).map_err(|e| format!("the move at {at:.2} s: {e}"))?;
            }
        }

        Ok(Self {
            runner: CueRunner::new(cues, 1.0 / config.fps as f64),
            config,
            frame: -warmup - 1,
            warmup,
            fresh: false,
            drawn: false,
            frame_input: egui::RawInput::default(),
            skipped: 0,
            awaiting_shot: false,
            shot: Arc::default(),
            ffmpeg: None,
            wav: None,
            audio: Vec::new(),
            samples_done: 0,
            grid,
            // From the start of the block the warmup's first sample is in
            rendered: -(-grid).rem_euclid(BLOCK_SIZE as i64),
            carry: Vec::new(),
            midi,
            next_midi: 0,
            stills: Vec::new(),

            finished: false,
            size_checked: false,
            input: None,
            input_frame: Vec::new(),
            played: false,
        })
    }

    pub fn config(&self) -> &CaptureConfig {
        &self.config
    }

    /// Seconds per frame.
    pub fn dt(&self) -> f64 {
        1.0 / self.config.fps as f64
    }

    /// The script time at the start of the current frame.
    pub fn time(&self) -> f64 {
        self.frame as f64 * self.dt()
    }

    fn recording(&self) -> bool {
        self.frame >= 0
    }

    /// Whether this UI pass is a new frame.
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Replaces the frame's input with the script's: called before each UI
    /// pass. A pass only becomes a new frame once the last frame's
    /// screenshot is in hand. `locate` finds a module's part on screen.
    pub fn prepare_input(
        &mut self,
        ctx: &egui::Context,
        raw: &mut egui::RawInput,
        mut locate: impl FnMut(&Part) -> Result<Pos2, String>,
    ) {
        // When a redraw is overdue (250 ms on the wall clock, as on a busy
        // machine), eframe runs a pass of the app's logic alone, and asks for
        // its input too. A frame begun in that pass was never drawn, so the
        // next pass gets the same input rather than the frame after: else
        // that frame goes unfilmed, and the cables' history, ticked once
        // instead of twice, differs a pixel here and there for seconds (#492)
        if self.fresh && !self.drawn {
            raw.events.clone_from(&self.frame_input.events);
            raw.hovered_files.clone_from(&self.frame_input.hovered_files);
            raw.dropped_files.clone_from(&self.frame_input.dropped_files);
            raw.focused = true;
            raw.predicted_dt = self.dt() as f32;
            raw.time = Some(self.ui_time());
            return;
        }
        let shot = self.shot.lock().unwrap_or_else(|e| e.into_inner()).take();
        raw.events.clear();
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        // A drag lasts across passes, as a real one does, and so does a held
        // key: a still is drawn in the pass after its cues, so without it Alt
        // held over a face never shows
        self.runner.hover_into(raw);
        raw.events.push(egui::Event::ModifiersChanged(self.runner.modifiers()));
        raw.focused = true;
        raw.predicted_dt = self.dt() as f32;

        if self.awaiting_shot {
            match shot {
                Some(image) => {
                    self.awaiting_shot = false;
                    self.take_frame(&image);
                }
                None => {
                    // A repaint between frames: same time, no input
                    self.fresh = false;
                    raw.time = Some(self.ui_time());
                    return;
                }
            }
        }
        if self.finished {
            // Still the capture's clock, not eframe's: at a low frame rate
            // the script runs ahead of the wall, and egui's pointer history
            // panics if time goes back (#224)
            self.fresh = false;
            raw.time = Some(self.ui_time());
            return;
        }

        self.frame += 1;
        self.fresh = true;
        self.drawn = false;
        self.skipped = 0;
        raw.time = Some(self.ui_time());

        // Draw the interface at the configured scale, whatever the display's
        if let Some(native) = raw.viewport().native_pixels_per_point {
            let zoom = self.config.ppp / native;
            if (ctx.zoom_factor() - zoom).abs() > 1e-4 {
                ctx.set_zoom_factor(zoom);
            }
        }
        if self.frame == -self.warmup {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.config.size_in_points()));
        }

        self.fire_cues(raw, &mut locate);
        // A move's part is followed as the view glides
        self.runner.follow_aim(&mut locate);
        raw.events.push(egui::Event::ModifiersChanged(self.runner.modifiers()));
        self.runner.animate(self.time(), self.time() + self.dt(), raw);
        self.frame_input.events.clone_from(&raw.events);
        self.frame_input.hovered_files.clone_from(&raw.hovered_files);
        self.frame_input.dropped_files.clone_from(&raw.dropped_files);
    }

    /// egui's clock, which mustn't go below zero during warmup.
    pub fn ui_time(&self) -> f64 {
        (self.frame + self.warmup + 1) as f64 * self.dt()
    }

    /// Steps on to the next frame without drawing it, if nobody could
    /// tell: a long warmup fast-forwards until
    /// it's `FAST_FORWARD_MARGIN` from the first recorded frame, so long as
    /// the frame's cues only reach the sound (notes, params, `play`) and
    /// nothing on screen is moving. `settled` is the app's word that its own
    /// view isn't (a fit or a dive). The app then runs the frame's actions
    /// and renders its audio as for a drawn one. Returns whether it did.
    pub fn skip_frame(&mut self, settled: bool) -> bool {
        let next = self.frame + 1;
        let end = (next + 1) as f64 * self.dt();
        let ready = self.fresh
            && settled
            && !self.finished
            && self.frame + self.warmup + 1 >= WARMUP_FRAMES
            && end <= -FAST_FORWARD_MARGIN
            && (self.skipped as f64) < FAST_FORWARD_STRIDE * self.config.fps as f64;
        let (still, heard_only) = (self.runner.is_still(), self.runner.only_heard_before(end));
        if !(ready && still && heard_only) {
            return false;
        }
        self.frame = next;
        self.skipped += 1;
        // None of these cues touch the pass's input, or aim at anything
        self.fire_cues(&mut egui::RawInput::default(), &mut |_| Err("isn't drawn".into()));
        true
    }

    /// Fires the cues that fall inside this frame, and the MIDI that falls
    /// inside the audio the engine renders for it.
    fn fire_cues(&mut self, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        self.fire_midi();
        let end = self.time() + self.dt();
        // What only a recording does, it does here
        let (config, stills, input, rendered, finished) = (&self.config, &mut self.stills, &mut self.input, self.rendered, &mut self.finished);
        self.runner.fire(end, raw, locate, &mut |_, cue| match cue {
            Cue::Still(name) => {
                stills.push(name);
                None
            }
            Cue::Layout(name) => Some(CaptureAction::Layout(config.out_dir.join(format!("{name}.json")))),
            Cue::Input(path) => match read_input_at(&path, config.sample_rate) {
                Ok((audio, rate)) => {
                    if rate != config.sample_rate {
                        eprintln!("capture: {} is {} Hz, resampled to {} Hz", path.display(), rate, config.sample_rate);
                    }
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    let channels = audio.channels();
                    // From the next sample the engine renders: a cue's offset isn't kept
                    *input = Some((audio, rendered));
                    Some(CaptureAction::InputFile(name, channels))
                }
                Err(e) => {
                    eprintln!("capture: {}: {}", path.display(), e);
                    None
                }
            },
            Cue::End => {
                *finished = true;
                None
            }
            _ => None,
        });
        if self.runner.failed() {
            self.finished = true;
        }
    }

    /// The pan the graph should have this frame, given its current pan.
    pub fn take_pan(&mut self, current: Vec2) -> Option<Vec2> {
        self.runner.take_pan(self.time() + self.dt(), current)
    }

    /// The zoom factor to apply this frame (1.0 for none).
    pub fn take_zoom(&mut self) -> f32 {
        self.runner.take_zoom()
    }

    /// The app's work for this frame.
    pub fn take_actions(&mut self) -> Vec<CaptureAction> {
        self.runner.take_actions()
    }

    /// The parameter values the script sets this frame, given a way to read
    /// a parameter's current value.
    pub fn param_values(&mut self, current: impl FnMut(&str, Which, &str) -> Option<f32>) -> Vec<CaptureAction> {
        self.runner.param_values(self.time() + self.dt(), current)
    }

    /// How many samples this frame covers. Warmup frames render at the
    /// same rate as recorded ones, so the music is already in time.
    pub fn samples_this_frame(&self) -> usize {
        let frames_done = (self.frame + self.warmup + 1).max(0) as f64;
        let end = (frames_done * self.config.sample_rate as f64 / self.config.fps as f64).round() as u64;
        end.saturating_sub(self.samples_done) as usize
    }

    /// How far the engine renders for this frame: to the first block edge
    /// at or past its end.
    fn horizon(&self) -> i64 {
        let end = (self.samples_done + self.samples_this_frame() as u64) as i64;
        self.rendered.max(end + (self.grid - end).rem_euclid(BLOCK_SIZE as i64))
    }

    /// Sends the script's MIDI that falls before the horizon, placed by
    /// sample in the audio the engine renders next. The capture knows its
    /// MIDI ahead of time, so an event in the part of a block that runs on
    /// past a frame still lands on its sample.
    fn fire_midi(&mut self) {
        let horizon = self.horizon();
        let rate = self.config.sample_rate as f64;
        while let Some(&(at, event)) = self.midi.get(self.next_midi) {
            let sample = self.grid + (at * rate).round() as i64;
            if sample >= horizon {
                break;
            }
            self.next_midi += 1;
            let offset = (sample - self.rendered).max(0) as u32;
            self.runner.push(CaptureAction::Midi { event, offset });
        }
    }

    /// Renders the audio this frame covers with `render` (the engine), and
    /// keeps it. The engine runs in whole blocks counted from the first
    /// recorded frame, cut where `render` cuts them, so a film's sound is
    /// `render`'s sample for sample: the block that runs past a frame's end
    /// is rendered whole, and the next frame takes the rest. `midi` is
    /// placed from the first sample rendered, as `fire_midi` placed it, and
    /// the engine hears the share of an `input` file that it renders.
    pub fn render_audio(&mut self, midi: &mut [crate::dsp::MidiEvent], render: impl FnOnce(&mut [f32], &mut [crate::dsp::MidiEvent], InputAudio<'_>)) {
        let start = self.rendered;
        let horizon = self.horizon();
        let frames = (horizon - start) as usize;
        if frames > 0 {
            self.audio.clear();
            self.audio.resize(frames * 2, 0.0);
            self.input_frame.clear();
            let mut channels = 0;
            if let Some((file, from)) = &self.input {
                let len = file.frames() as i64;
                let (a, b) = ((start - from).clamp(0, len) as usize, (horizon - from).clamp(0, len) as usize);
                self.input_frame.extend_from_slice(file.frames_between(a, b));
                channels = file.channels();
            }
            let input = InputAudio::interleaved(&self.input_frame, channels);
            render(&mut self.audio, midi, input);
            self.rendered = horizon;
            // Only the warmup's first block starts before the frames do
            let stale = (self.samples_done as i64 - start).max(0) as usize;
            self.carry.extend_from_slice(&self.audio[stale * 2..]);
        }

        let taken = self.samples_this_frame() * 2;
        self.samples_done += (taken / 2) as u64;
        if self.recording() {
            if let Some(wav) = self.wav.as_mut() {
                for &s in &self.carry[..taken] {
                    let _ = wav.write_sample(s);
                }
            }
        }
        self.carry.drain(..taken);
    }

    /// Draws the pointer, which the screenshot wouldn't otherwise show.
    pub fn draw_cursor(&self, ctx: &egui::Context) {
        self.runner.draw_cursor(ctx);
    }

    /// Asks for this frame's screenshot, after the app has drawn it.
    /// Notes whether the transport runs during this frame's audio, however
    /// it was started: a `play` cue, a click on Play, a key, a take.
    pub fn note_transport(&mut self, playing: bool) {
        self.played |= playing && self.recording();
    }

    pub fn end_frame(&mut self, ctx: &egui::Context) {
        self.check_aim(ctx);
        if self.fresh {
            let (slot, ctx2) = (self.shot.clone(), ctx.clone());
            ctx.request_screenshot(move |image| {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(image);
                ctx2.request_repaint();
            });
            self.awaiting_shot = true;
            self.drawn = true;
        }
        ctx.request_repaint();
    }

    /// Warns about a press that just missed a control: a script aimed at
    /// one `--ppp` and filmed at another, say, since the layout shifts a
    /// little with the scale as text rows snap to whole pixels. Otherwise the
    /// press quietly lands on the module or canvas behind.
    fn check_aim(&mut self, ctx: &egui::Context) {
        for (at, cue, p) in self.runner.take_presses() {
            let Some(layer) = ctx.layer_id_at(p) else { continue };
            let (hit, nearby): (Vec<Rect>, Vec<Rect>) = ctx.viewport(|v| {
                let clickable = |w: &&egui::WidgetRect| w.enabled && (w.sense.senses_click() || w.sense.senses_drag());
                let hovered = &v.interact_widgets.hovered;
                let widgets = || v.prev_pass.widgets.get_layer(layer).filter(clickable);
                (
                    widgets().filter(|w| hovered.contains(&w.id)).map(|w| w.interact_rect).collect(),
                    widgets().filter(|w| !hovered.contains(&w.id)).map(|w| w.interact_rect).collect(),
                )
            });
            if let Some(missed) = near_miss(p, &hit, &nearby) {
                let gap = missed.clamp(p) - p;
                let side = match (gap.x.abs() > gap.y.abs(), gap.x > 0.0, gap.y > 0.0) {
                    (true, true, _) => "right of",
                    (true, false, _) => "left of",
                    (false, _, true) => "below",
                    (false, _, false) => "above",
                };
                eprintln!(
                    "capture: the {cue} at {at:.2} s, ({:.0}, {:.0}), misses a {:.0}x{:.0} control {:.1} pt {side} it. \
                     The layout shifts a little with --ppp: aim from a still filmed at this one ({})",
                    p.x,
                    p.y,
                    missed.width(),
                    missed.height(),
                    gap.length(),
                    self.config.ppp
                );
            }
        }
    }

    /// Writes a finished frame to the video (and as a still if one was cued).
    fn take_frame(&mut self, image: &egui::ColorImage) {
        let [w, h] = self.config.size;
        if image.size != [w as usize, h as usize] {
            if self.recording() {
                eprintln!(
                    "capture: frame is {}x{}, expected {}x{}; is the window too big for the screen?",
                    image.size[0], image.size[1], w, h
                );
                self.finished = true;
            }
            return;
        }
        if !self.size_checked {
            self.size_checked = true;
            eprintln!("capture: window is {}x{}, recording", w, h);
        }
        if !self.recording() {
            return;
        }
        if let Some(stdin) = self.ffmpeg.as_mut().and_then(|f| f.stdin.as_mut()) {
            if let Err(e) = stdin.write_all(image.as_raw()) {
                eprintln!("capture: ffmpeg write failed: {}", e);
                self.finished = true;
            }
        }
        for name in std::mem::take(&mut self.stills) {
            if let Err(e) = write_ppm(&self.config.out_dir.join(format!("{}.ppm", name)), image) {
                eprintln!("capture: still {} failed: {}", name, e);
            }
        }
    }

    /// Closes the video and audio files. Returns true once everything is on
    /// disk and the app can close.
    pub fn finish(&mut self) -> bool {
        if !self.finished || self.awaiting_shot {
            return false;
        }
        if let Some(wav) = self.wav.take() {
            let _ = wav.finalize();
        }
        if let Some(mut ffmpeg) = self.ffmpeg.take() {
            drop(ffmpeg.stdin.take());
            let _ = ffmpeg.wait();
            eprintln!(
                "capture: {} frames, {:.2} s, written to {}",
                self.frame,
                self.frame as f64 * self.dt(),
                self.config.out_dir.display()
            );
            if !self.played {
                eprintln!("capture: the script never plays the transport, so the audio is silent (add `0.0 play`)");
            }
        }
        true
    }
}

/// An arrow pointer with its tip at `p`, as a capture draws the script's
/// and a lesson its ghost, faded to `opacity`.
pub fn draw_pointer(painter: &egui::Painter, p: Pos2, opacity: f32) {
    let shape: Vec<Pos2> = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.2, 19.6), (9.6, 18.6), (6.7, 12.3), (12.2, 12.3)]
        .iter()
        .map(|&(x, y)| p + Vec2::new(x, y))
        .collect();
    let shadow: Vec<Pos2> = shape.iter().map(|q| *q + Vec2::new(1.0, 1.5)).collect();
    painter.add(egui::Shape::convex_polygon(shadow, egui::Color32::from_black_alpha(90).gamma_multiply(opacity), egui::Stroke::NONE));
    // The arrow isn't convex; a closed path fills it correctly
    painter.add(egui::Shape::Path(egui::epaint::PathShape {
        points: shape,
        closed: true,
        fill: egui::Color32::WHITE.gamma_multiply(opacity),
        stroke: egui::epaint::PathStroke::new(1.2_f32, egui::Color32::from_gray(20).gamma_multiply(opacity)),
    }));
}

/// The control a press at `at` just misses, if it missed one: something
/// clickable within `NEAR_MISS` of it that egui didn't find under the
/// pointer, and much smaller than anything it did (the module or canvas a
/// near miss lands on instead). `hit` and `nearby` are the clickable
/// widgets under the pointer and the rest of its layer's.
fn near_miss(at: Pos2, hit: &[Rect], nearby: &[Rect]) -> Option<Rect> {
    let landed = hit.iter().map(|r| r.area()).fold(f32::INFINITY, f32::min);
    nearby
        .iter()
        .filter(|r| !r.contains(at) && r.area() * 4.0 < landed)
        .map(|r| (r.distance_to_pos(at), *r))
        .filter(|(d, _)| *d <= NEAR_MISS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, r)| r)
}

/// Saves an image as a binary PPM, which any image tool can read.
fn write_ppm(path: &Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    write!(out, "P6\n{} {}\n255\n", image.size[0], image.size[1])?;
    for px in &image.pixels {
        out.write_all(&[px.r(), px.g(), px.b()])?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_is_lossless_unless_asked() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let c = CaptureConfig::from_args(&args("soba p.json --capture s.txt")).unwrap().unwrap();
        assert_eq!((c.crf, c.preset.as_str()), (0, "ultrafast"));
        let c = CaptureConfig::from_args(&args("soba p.json --capture s.txt --crf 12 --preset fast")).unwrap().unwrap();
        assert_eq!((c.crf, c.preset.as_str()), (12, "fast"));
        assert!(CaptureConfig::from_args(&args("soba --capture s.txt --crf high")).is_err());
        assert!(CaptureConfig::is_option("--crf") && CaptureConfig::is_option("--preset"));
    }

    #[test]
    fn frames_take_their_audio_from_whole_blocks_on_renders_grid() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let config = CaptureConfig::from_args(&args("soba p.json --capture s.txt --fps 30")).unwrap().unwrap();
        // 0.5 s is 93.75 blocks: the note falls in the block that runs on past frame 14
        let mut c = Capture::from_script(config, "0.0 play\n0.5 note 60 100 0.25\n1.0 end\n").unwrap();
        let block = BLOCK_SIZE as i64;
        let mut notes = Vec::new();
        while c.frame < 30 {
            c.frame += 1;
            c.fire_cues(&mut egui::RawInput::default(), &mut |_| Err(String::new()));
            let (frame, start, grid) = (c.frame, c.rendered, c.grid);
            for action in c.take_actions() {
                if let CaptureAction::Midi { offset, .. } = action {
                    notes.push((frame, start + offset as i64 - grid));
                }
            }
            c.render_audio(&mut [], |buffer, _, _| {
                let frames = buffer.len() / 2;
                assert_eq!(((start - grid).rem_euclid(block), frames as i64 % block), (0, 0), "frame {frame}");
                // Each sample says where it is
                for (i, pair) in buffer.chunks_mut(2).enumerate() {
                    pair.fill((start + i as i64) as f32);
                }
            });
            assert!(c.carry.len() < BLOCK_SIZE * 2);
            if let Some(&next) = c.carry.first() {
                assert_eq!(next, c.samples_done as f32, "frame {frame} left a gap");
            }
        }
        assert_eq!(c.samples_done as i64, c.grid + 31 * 1600);
        assert_eq!(notes, [(14, 24_000), (22, 36_000)]);
    }

    #[test]
    fn a_frame_no_pass_drew_is_given_again() {
        // #492: eframe runs a pass of logic alone when a redraw is overdue,
        // and it still asks for the input
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let config = CaptureConfig::from_args(&args("soba p.json --capture s.txt --fps 30")).unwrap().unwrap();
        let mut c = Capture::from_script(config, "-1.5 cursor on\n-1.5 move 400 300 1.0\n1.0 end\n").unwrap();
        let ctx = egui::Context::default();
        let pass = |c: &mut Capture| {
            let mut raw = egui::RawInput::default();
            c.prepare_input(&ctx, &mut raw, |_| Err(String::new()));
            (c.frame, raw.time, raw.events)
        };
        let drawn = |c: &mut Capture| {
            c.end_frame(&ctx);
            *c.shot.lock().unwrap() = Some(Arc::new(egui::ColorImage::filled([4, 4], egui::Color32::BLACK)));
        };
        let first = pass(&mut c);
        drawn(&mut c);
        // A logic-only pass begins the next frame...
        let begun = pass(&mut c);
        assert_eq!(begun.0, first.0 + 1);
        // ...so the pass that draws it is still that frame, with its input
        let again = pass(&mut c);
        assert_eq!(again, begun);
        drawn(&mut c);
        assert_eq!(pass(&mut c).0, begun.0 + 1);
    }

    #[test]
    fn a_press_just_off_a_button_is_a_near_miss() {
        // #190: Soba Speaks at ppp 2, where the Sampler's file button sits
        // 7 pt lower than at ppp 1, so a press aimed at ppp 1 lands on the
        // module around it
        let rect = |x0, y0, x1, y1| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        let module = rect(270.0, 150.9, 500.0, 874.0);
        let canvas = rect(0.0, 76.9, 1200.0, 874.0);
        let button = rect(305.0, 601.9, 417.0, 613.9);
        let knob = rect(285.0, 630.0, 320.0, 665.0);
        let at = Pos2::new(345.0, 600.0);
        assert_eq!(near_miss(at, &[module, canvas], &[button, knob]), Some(button));
        // On the button, nothing to say
        assert_eq!(near_miss(Pos2::new(345.0, 607.0), &[module, canvas, button], &[knob]), None);
        // Nowhere near a control: a press on the module is meant
        assert_eq!(near_miss(Pos2::new(345.0, 400.0), &[module, canvas], &[button, knob]), None);
        // A press on a small control beside a bigger one is meant too
        assert_eq!(near_miss(Pos2::new(300.0, 621.0), &[module, rect(290.0, 616.0, 310.0, 626.0)], &[button]), None);
        // Bare canvas, with a module's port just off the pointer
        let port = rect(265.0, 300.0, 275.0, 310.0);
        assert_eq!(near_miss(Pos2::new(262.0, 305.0), &[canvas], &[port]), Some(port));
    }
}
