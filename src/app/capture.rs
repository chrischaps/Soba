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
//! knobs, MIDI notes reach Poly MIDI and MIDI Note modules at the exact
//! sample they're scheduled for. Real mouse and keyboard input is ignored
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
//! 1.0  chord 60,64,67 90 3.0
//! 1.0  midi start                   # MIDI Start, Stop or Continue
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
//! 4.0  view 120 -40 2.0             # pan the graph to an offset over 2 s
//! 4.0  zoom 1.4 2.0                 # zoom the graph by a factor over 2 s
//! 4.0  fit                          # frame the whole level, as F does
//! 4.0  fit 1.5                      # ...gliding there over 1.5 s
//! 4.0  aim 1200 640 1.5 2.0         # glide to centre patch point (1200,
//!                                   # 640) at zoom 1.5 over 2 s, on the
//!                                   # level on show (a layout's patch space)
//! 4.0  enter Bells                  # dive into a group through its
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
//! 9.0  end
//! ```

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use eframe::egui;
use egui::{Pos2, Rect, Vec2};

use super::capture_aim::{self, Part, Spot, Which};
use crate::dsp::InputAudio;
use crate::engine::{read_wav_at, MidiEvent, StereoBuffer, BLOCK_SIZE};

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

/// A cue from the script.
#[derive(Debug, Clone)]
enum Cue {
    Play(bool),
    KeyDown(egui::Key, egui::Modifiers),
    KeyUp(egui::Key, egui::Modifiers),
    Text(String),
    Note { note: u8, velocity: u8, on: bool },
    /// Any other MIDI message, such as a clock tick.
    Midi(MidiEvent),
    Param { module: String, which: Which, input: String, value: f32, dur: f64 },
    Cursor(bool),
    Move { to: Spot, dur: f64 },
    /// The pointer leaves the window.
    PointerOff,
    Button(bool),
    /// The secondary button, pressed or released.
    SecondaryButton(bool),
    /// A turn of the mouse wheel, in points, with Ctrl held or not.
    Wheel { delta: f32, ctrl: bool },
    View { pan: Vec2, dur: f64 },
    Zoom { factor: f32, dur: f64 },
    Camera { offset: Vec2, scale: f32, dur: f64 },
    /// Frame the whole level, over this many seconds.
    Fit(f64),
    /// Centre a patch point at a zoom, over this many seconds.
    Aim { at: Pos2, zoom: f32, dur: f64 },
    /// Dive into the group of this name (or path), landing on all of it or
    /// aimed at a patch point inside at a zoom, or back out of one.
    Enter(String, Option<(Pos2, f32)>),
    Leave,
    Still(String),
    /// Save where the modules are drawn, under this name.
    Layout(String),
    /// Play a WAV file into Audio Input modules.
    Input(PathBuf),
    /// Drag a file over the window from the desktop, or give the drag up.
    Hover(Option<PathBuf>),
    /// Drop the file dragged over the window where the pointer is.
    Drop,
    End,
}

/// Something the app has to do for the script this frame, beyond input.
#[derive(Debug, Clone)]
pub enum CaptureAction {
    Play(bool),
    /// A MIDI event, `offset` samples into the audio the engine renders next.
    Midi { event: MidiEvent, offset: u32 },
    /// Set a module's input to a value in real units.
    SetParam { module: String, which: Which, input: String, value: f32 },
    /// Audio Input modules are now hearing this file.
    InputFile(String),
    /// Write where each module is drawn, in patch space, to this file.
    Layout(PathBuf),
    /// Frame the whole level, over this many seconds (0 cuts).
    Fit(f64),
    /// Centre a patch point at a zoom, over this many seconds (0 cuts).
    Aim { at: Pos2, zoom: f32, seconds: f64 },
    /// Dive into the group of this name, or path from the top, landing on
    /// all of it or aimed at a patch point inside at a zoom.
    Enter(String, Option<(Pos2, f32)>),
    /// Dive back out of the group on show.
    Leave,
}

/// A value easing from one point to another over a span of time.
#[derive(Debug, Clone, Copy)]
struct Glide<T> {
    from: T,
    to: T,
    start: f64,
    dur: f64,
}

impl<T: Copy + std::ops::Add<Output = T> + std::ops::Sub<Output = T> + std::ops::Mul<f32, Output = T>> Glide<T> {
    /// Where the glide is at `t`, eased in and out.
    fn at(&self, t: f64) -> T {
        self.from + (self.to - self.from) * ease(self.progress(t))
    }

    fn progress(&self, t: f64) -> f32 {
        if self.dur <= 0.0 { 1.0 } else { ((t - self.start) / self.dur).clamp(0.0, 1.0) as f32 }
    }

    fn done(&self, t: f64) -> bool {
        self.progress(t) >= 1.0
    }
}

/// A camera move: scale eases in log space, so a zoom feels even, and the
/// point at the centre of the view travels a straight line across the patch.
#[derive(Debug, Clone, Copy)]
struct CameraGlide {
    from: (f32, Vec2),
    to: (f32, Vec2),
    start: f64,
    dur: f64,
}

impl CameraGlide {
    fn progress(&self, t: f64) -> f32 {
        ((t - self.start) / self.dur).clamp(0.0, 1.0) as f32
    }

    fn at(&self, t: f64) -> (f32, Vec2) {
        let p = ease(self.progress(t));
        let scale = (self.from.0.ln() + (self.to.0.ln() - self.from.0.ln()) * p).exp();
        // The offset scales with the zoom (see `animate`), so offset / scale
        // is where the view's centre sits in the patch. Easing that, not the
        // offset itself, keeps a deep pull-back from swinging out past the
        // edge of the patch on its way to the wide shot
        let (a, b) = (self.from.1 / self.from.0, self.to.1 / self.to.0);
        (scale, (a + (b - a) * p) * scale)
    }
}

/// Smoothstep, the motion of a hand that starts and stops gently.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// A parameter turning from one value to another.
#[derive(Debug, Clone)]
struct Ramp {
    module: String,
    which: Which,
    input: String,
    /// Filled in from the parameter's value when the ramp starts.
    from: Option<f32>,
    to: f32,
    start: f64,
    dur: f64,
}

/// The running capture.
pub struct Capture {
    config: CaptureConfig,
    cues: Vec<(f64, Cue)>,
    next_cue: usize,
    /// The frame being drawn; negative while warming up.
    frame: i64,
    /// How many frames the warmup lasts.
    warmup: i64,
    /// Whether this UI pass is a new frame (rather than a repaint while
    /// waiting for the last frame's screenshot).
    fresh: bool,
    /// Frames stepped without drawing since the last drawn one.
    skipped: i64,
    /// A screenshot has been asked for and hasn't arrived.
    awaiting_shot: bool,
    actions: Vec<CaptureAction>,
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
    cursor_visible: bool,
    /// Where the pointer is; `None` until the script first moves it, so
    /// nothing is hovered by accident.
    pointer: Option<Pos2>,
    pointer_glide: Option<Glide<Vec2>>,
    /// The part the pointer is gliding to, followed as the view moves.
    pointer_aim: Option<Part>,
    button_down: bool,
    /// Modifier keys held with the last key pressed.
    modifiers: egui::Modifiers,
    ramps: Vec<Ramp>,
    pan_glide: Option<Glide<Vec2>>,
    /// Zoom still to apply: (log of the factor left, frames left).
    zoom_left: Option<(f32, f64, f64)>,
    /// The virtual camera, relative to the view when it was first moved
    /// or last fitted: the screen is scaled by `scale` about the editor's
    /// centre, then shifted by `offset` points.
    camera: (f32, Vec2),
    camera_glide: Option<CameraGlide>,
    /// Pan to add this frame, from the camera.
    pending_pan_delta: Vec2,
    pending_zoom: f32,

    finished: bool,
    size_checked: bool,
    /// A WAV playing into Audio Input modules, and the sample it started at.
    input: Option<(StereoBuffer, i64)>,
    /// A file dragged over the window, as from the desktop.
    hovering: Option<PathBuf>,
    /// Where this frame's presses and right-clicks landed, and their cue
    /// times, to check against what egui found under them.
    presses: Vec<(f64, &'static str, Pos2)>,
    /// The share of `input` the engine renders this frame.
    input_frame: StereoBuffer,
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
        let cues = parse_script(text)?;
        let earliest = cues.first().map_or(0.0, |c| c.0).min(0.0);
        let warmup = WARMUP_FRAMES.max((-earliest * config.fps as f64).ceil() as i64 + 1);
        let grid = (warmup as f64 * config.sample_rate as f64 / config.fps as f64).round() as i64;
        let (midi, cues): (Vec<_>, Vec<_>) = cues.into_iter().partition(|(_, cue)| matches!(cue, Cue::Note { .. } | Cue::Midi(_)));
        let midi = midi
            .into_iter()
            .map(|(at, cue)| match cue {
                Cue::Note { note, velocity, on: true } => (at, MidiEvent::NoteOn { channel: 0, note, velocity }),
                Cue::Note { note, on: false, .. } => (at, MidiEvent::NoteOff { channel: 0, note, velocity: 0 }),
                Cue::Midi(event) => (at, event),
                _ => unreachable!(),
            })
            .collect();
        for (at, cue) in &cues {
            if let Cue::Move { to: Spot::Part(part), .. } = cue {
                capture_aim::check(part).map_err(|e| format!("the move at {at:.2} s: {e}"))?;
            }
        }

        Ok(Self {
            config,
            cues,
            next_cue: 0,
            frame: -warmup - 1,
            warmup,
            fresh: false,
            skipped: 0,
            awaiting_shot: false,
            actions: Vec::new(),
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
            cursor_visible: false,
            pointer: None,
            pointer_glide: None,
            pointer_aim: None,
            button_down: false,
            modifiers: egui::Modifiers::default(),
            ramps: Vec::new(),
            pan_glide: None,
            zoom_left: None,
            camera: (1.0, Vec2::ZERO),
            camera_glide: None,
            pending_pan_delta: Vec2::ZERO,
            pending_zoom: 1.0,

            finished: false,
            size_checked: false,
            input: None,
            input_frame: StereoBuffer::default(),
            hovering: None,
            presses: Vec::new(),
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
        let mut shot = None;
        for event in raw.events.drain(..) {
            if let egui::Event::Screenshot { image, .. } = event {
                shot = Some(image);
            }
        }
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        // A drag lasts across passes, as a real one does, and so does a held
        // key: a still is drawn in the pass after its cues, so without it Alt
        // held over a face never shows
        self.hover_into(raw);
        raw.modifiers = self.modifiers;
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
        // A move's part is followed as the view glides, but not by a drag,
        // which might be what moves it
        if let (Some(part), Some(glide), false) = (&self.pointer_aim, self.pointer_glide.as_mut(), self.button_down) {
            if let Ok(to) = locate(part) {
                glide.to = to.to_vec2();
            }
        }
        raw.modifiers = self.modifiers;
        self.animate(raw);
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
        let still = self.pointer_glide.is_none()
            && self.pan_glide.is_none()
            && self.zoom_left.is_none()
            && self.camera_glide.is_none()
            && self.pending_pan_delta == Vec2::ZERO
            && self.pending_zoom == 1.0
            && !self.button_down
            && self.hovering.is_none()
            && self.modifiers.is_none();
        let heard_only = self.cues[self.next_cue..].iter().take_while(|(at, _)| *at < end - 1e-9).all(|(_, cue)| {
            matches!(
                cue,
                Cue::Play(_) | Cue::Note { .. } | Cue::Midi(_) | Cue::Param { .. } | Cue::Cursor(_) | Cue::Still(_) | Cue::Input(_)
            )
        });
        if !(ready && still && heard_only) {
            return false;
        }
        self.frame = next;
        self.skipped += 1;
        // None of these cues touch the pass's input, or aim at anything
        self.fire_cues(&mut egui::RawInput::default(), &mut |_| Err("isn't drawn".into()));
        true
    }

    /// Tells the pass which file is dragged over the window, if one is.
    fn hover_into(&self, raw: &mut egui::RawInput) {
        raw.hovered_files.clear();
        if let Some(path) = &self.hovering {
            raw.hovered_files.push(egui::HoveredFile { path: Some(path.clone()), ..Default::default() });
        }
    }

    /// Fires the cues that fall inside this frame, and the MIDI that falls
    /// inside the audio the engine renders for it.
    fn fire_cues(&mut self, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        self.fire_midi();
        let t = self.time();
        let end = t + self.dt();
        while let Some((at, cue)) = self.cues.get(self.next_cue).cloned() {
            if at >= end - 1e-9 {
                break;
            }
            self.next_cue += 1;
            match cue {
                Cue::Play(on) => self.actions.push(CaptureAction::Play(on)),
                Cue::KeyDown(key, modifiers) | Cue::KeyUp(key, modifiers) => {
                    let pressed = matches!(cue, Cue::KeyDown(..));
                    self.modifiers = if pressed { modifiers } else { egui::Modifiers::default() };
                    raw.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    });
                }
                Cue::Text(text) => raw.events.push(egui::Event::Text(text)),
                // Kept apart, for `fire_midi`
                Cue::Note { .. } | Cue::Midi(_) => {}
                Cue::Param { module, which, input, value, dur } => self.ramps.push(Ramp {
                    module,
                    which,
                    input,
                    from: None,
                    to: value,
                    start: at,
                    dur,
                }),
                Cue::Cursor(on) => self.cursor_visible = on,
                Cue::Move { to: spot, dur } => {
                    // Where a glide under way was headed, for a move by
                    let heading = self.pointer_glide.map(|g| g.to.to_pos2()).or(self.pointer);
                    let to = match &spot {
                        Spot::Point(p) => Ok(*p),
                        Spot::By(by) => heading.map(|p| p + *by).ok_or_else(|| "has no pointer to move from".to_string()),
                        Spot::Part(part) => locate(part),
                    };
                    let to = match to {
                        Ok(to) => to,
                        Err(e) => {
                            eprintln!("capture: the move at {at:.2} s {e}");
                            self.finished = true;
                            continue;
                        }
                    };
                    // The first move comes in from below and to the right
                    let from = self.pointer.unwrap_or(to + Vec2::new(150.0, 200.0));
                    self.pointer = Some(from);
                    self.pointer_glide = Some(Glide { from: from.to_vec2(), to: to.to_vec2(), start: at, dur });
                    self.pointer_aim = match spot {
                        Spot::Part(part) => Some(part),
                        _ => None,
                    };
                }
                Cue::PointerOff => {
                    self.pointer = None;
                    self.pointer_glide = None;
                    self.pointer_aim = None;
                }
                Cue::Button(down) => {
                    self.button_down = down;
                    if let (true, Some(p)) = (down, self.pointer) {
                        self.presses.push((at, "press", p));
                    }
                    raw.events.push(egui::Event::PointerButton {
                        pos: self.pointer.unwrap_or_default(),
                        button: egui::PointerButton::Primary,
                        pressed: down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                Cue::SecondaryButton(down) => {
                    if let (true, Some(p)) = (down, self.pointer) {
                        self.presses.push((at, "rclick", p));
                    }
                    raw.events.push(egui::Event::PointerButton {
                        pos: self.pointer.unwrap_or_default(),
                        button: egui::PointerButton::Secondary,
                        pressed: down,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                Cue::Wheel { delta, ctrl } => {
                    let modifiers = if ctrl { egui::Modifiers::CTRL } else { egui::Modifiers::default() };
                    raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, delta), modifiers });
                }
                Cue::View { pan, dur } => {
                    // `from` is filled in by the app, which knows the pan
                    self.pan_glide = Some(Glide { from: Vec2::NAN, to: pan, start: at, dur });
                }
                Cue::Camera { offset, scale, dur } => {
                    self.camera_glide = Some(CameraGlide {
                        from: self.camera,
                        to: (scale, offset),
                        start: at,
                        dur: dur.max(self.dt()),
                    });
                }
                Cue::Fit(dur) => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Fit(dur));
                }
                Cue::Aim { at, zoom, dur } => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Aim { at, zoom, seconds: dur });
                }
                Cue::Enter(name, aim) => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Enter(name, aim));
                }
                Cue::Leave => {
                    self.reset_camera();
                    self.actions.push(CaptureAction::Leave);
                }
                Cue::Zoom { factor, dur } => {
                    self.zoom_left = Some((factor.ln(), at, dur.max(self.dt())));
                }
                Cue::Still(name) => self.stills.push(name),
                Cue::Layout(name) => {
                    self.actions.push(CaptureAction::Layout(self.config.out_dir.join(format!("{name}.json"))));
                }
                Cue::Input(path) => match read_wav_at(&path, self.config.sample_rate) {
                    Ok((audio, rate)) => {
                        if rate != self.config.sample_rate {
                            eprintln!("capture: {} is {} Hz, resampled to {} Hz", path.display(), rate, self.config.sample_rate);
                        }
                        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                        self.actions.push(CaptureAction::InputFile(name));
                        // From the next sample the engine renders: a cue's offset isn't kept
                        self.input = Some((audio, self.rendered));
                    }
                    Err(e) => eprintln!("capture: {}: {}", path.display(), e),
                },
                Cue::Hover(path) => {
                    self.hovering = path;
                    self.hover_into(raw);
                }
                Cue::Drop => {
                    if let Some(path) = self.hovering.take() {
                        raw.dropped_files.push(egui::DroppedFile { path: Some(path), ..Default::default() });
                    }
                    self.hover_into(raw);
                }
                Cue::End => self.finished = true,
            }
        }
    }

    /// Camera moves from here on are from the view the app is about to
    /// move to (a fit, an aim, a dive), and one under way stops.
    fn reset_camera(&mut self) {
        self.camera = (1.0, Vec2::ZERO);
        self.camera_glide = None;
        self.pending_pan_delta = Vec2::ZERO;
        self.pending_zoom = 1.0;
    }

    /// Advances the pointer, pan and zoom glides for this frame.
    fn animate(&mut self, raw: &mut egui::RawInput) {
        let t = self.time() + self.dt();
        if let Some(glide) = self.pointer_glide {
            self.pointer = Some(glide.at(t).to_pos2());
            if glide.done(t) {
                self.pointer_glide = None;
                self.pointer_aim = None;
            }
        }
        // egui needs to know where the pointer is every frame
        let pointer = match self.pointer {
            Some(p) => egui::Event::PointerMoved(p),
            None => egui::Event::PointerGone,
        };
        raw.events.insert(0, pointer);

        if let Some((log_factor, start, dur)) = self.zoom_left {
            // This frame's share of the zoom, eased
            let p0 = ease(((self.time() - start) / dur).clamp(0.0, 1.0) as f32);
            let p1 = ease(((t - start) / dur).clamp(0.0, 1.0) as f32);
            let r = (log_factor * (p1 - p0)).exp();
            self.pending_zoom *= r;
            self.pending_pan_delta *= r;
            if p1 >= 1.0 {
                self.zoom_left = None;
            }
        }

        if let Some(glide) = self.camera_glide {
            // Zooming by r scales the screen, and so the camera's offset,
            // about the centre; panning then makes up the rest
            let (scale, offset) = glide.at(t);
            let r = scale / self.camera.0;
            self.pending_zoom *= r;
            // Pan still waiting (the app holds the camera while a fit
            // glides) is scaled by the zoom, as it would have been
            self.pending_pan_delta = self.pending_pan_delta * r + offset - self.camera.1 * r;
            self.camera = (scale, offset);
            if glide.progress(t) >= 1.0 {
                self.camera_glide = None;
            }
        }
    }

    /// The pan the graph should have this frame, given its current pan.
    pub fn take_pan(&mut self, current: Vec2) -> Option<Vec2> {
        let t = self.time() + self.dt();
        let delta = std::mem::take(&mut self.pending_pan_delta);
        let Some(glide) = self.pan_glide.as_mut() else {
            return (delta != Vec2::ZERO).then(|| current + delta);
        };
        if glide.from.x.is_nan() {
            glide.from = current;
        }
        let pan = glide.at(t);
        if glide.done(t) {
            self.pan_glide = None;
        }
        Some(pan + delta)
    }

    /// The zoom factor to apply this frame (1.0 for none).
    pub fn take_zoom(&mut self) -> f32 {
        std::mem::replace(&mut self.pending_zoom, 1.0)
    }

    /// The app's work for this frame.
    pub fn take_actions(&mut self) -> Vec<CaptureAction> {
        std::mem::take(&mut self.actions)
    }

    /// The parameter values the script sets this frame, given a way to read
    /// a parameter's current value.
    pub fn param_values(&mut self, mut current: impl FnMut(&str, Which, &str) -> Option<f32>) -> Vec<CaptureAction> {
        let t = self.time() + self.dt();
        let mut out = Vec::new();
        self.ramps.retain_mut(|ramp| {
            let from = *ramp.from.get_or_insert_with(|| current(&ramp.module, ramp.which, &ramp.input).unwrap_or(ramp.to));
            let p = if ramp.dur <= 0.0 { 1.0 } else { ease(((t - ramp.start) / ramp.dur).clamp(0.0, 1.0) as f32) };
            // Wide ranges of positive values (frequencies, times) turn evenly
            // in log space, as a knob with a log taper would
            let value = if from > 0.0 && ramp.to > 0.0 && (ramp.to / from).max(from / ramp.to) > 4.0 {
                (from.ln() + (ramp.to.ln() - from.ln()) * p).exp()
            } else {
                from + (ramp.to - from) * p
            };
            out.push(CaptureAction::SetParam {
                module: ramp.module.clone(),
                which: ramp.which,
                input: ramp.input.clone(),
                value,
            });
            p < 1.0
        });
        out
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
            self.actions.push(CaptureAction::Midi { event, offset });
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
            self.input_frame.left.clear();
            self.input_frame.right.clear();
            if let Some((file, from)) = &self.input {
                let len = file.left.len() as i64;
                let (a, b) = ((start - from).clamp(0, len) as usize, (horizon - from).clamp(0, len) as usize);
                self.input_frame.left.extend_from_slice(&file.left[a..b]);
                self.input_frame.right.extend_from_slice(&file.right[a..b]);
            }
            let input = InputAudio { left: &self.input_frame.left, right: &self.input_frame.right };
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
        let (true, Some(p)) = (self.cursor_visible, self.pointer) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("capture_cursor")));
        draw_pointer(&painter, p, 1.0);
        if self.button_down {
            painter.circle_stroke(p, 9.0, egui::Stroke::new(1.5, egui::Color32::from_white_alpha(120)));
        }
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
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.awaiting_shot = true;
        }
        ctx.request_repaint();
    }

    /// Warns about a press that just missed a control: a script aimed at
    /// one `--ppp` and filmed at another, say, since the layout shifts a
    /// little with the scale as text rows snap to whole pixels. Otherwise the
    /// press quietly lands on the module or canvas behind.
    fn check_aim(&mut self, ctx: &egui::Context) {
        for (at, cue, p) in std::mem::take(&mut self.presses) {
            let Some(layer) = ctx.layer_id_at(p) else { continue };
            let (hit, nearby): (Vec<Rect>, Vec<Rect>) = ctx.viewport(|v| {
                let clickable = |w: &&egui::WidgetRect| w.enabled && (w.sense.click || w.sense.drag);
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
        stroke: egui::epaint::PathStroke::new(1.2, egui::Color32::from_gray(20).gamma_multiply(opacity)),
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

/// Parses a script into time-ordered cues, expanding cues with a length
/// (key taps, notes, drags) into their start and end.
fn parse_script(text: &str) -> Result<Vec<(f64, Cue)>, String> {
    let mut cues = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        // `#` starts a comment at the start of a line or after a space;
        // `module#2` is a module reference
        let line = match line.find(" #") {
            Some(at) => &line[..at],
            None if line.trim_start().starts_with('#') => "",
            None => line,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let err = |msg: &str| format!("script line {}: {} ({})", line_no + 1, msg, line);
        let words: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> Result<f64, String> {
            words.get(i).ok_or_else(|| err("missing value"))?.parse::<f64>().map_err(|_| err("not a number"))
        };
        let opt = |i: usize| -> Result<f64, String> { if words.len() > i { num(i) } else { Ok(0.0) } };
        let t = num(0)?;
        let cue = *words.get(1).ok_or_else(|| err("missing cue"))?;
        // A key, with any modifiers before it: `Z`, `Ctrl+Shift+F`
        let key = |i: usize| -> Result<(egui::Key, egui::Modifiers), String> {
            let name = words.get(i).ok_or_else(|| err("missing key"))?;
            let mut parts: Vec<&str> = name.split('+').collect();
            let key = egui::Key::from_name(parts.pop().unwrap_or_default()).ok_or_else(|| err("unknown key"))?;
            let mut modifiers = egui::Modifiers::default();
            for part in parts {
                match part {
                    "Ctrl" => modifiers = modifiers | egui::Modifiers::COMMAND,
                    "Shift" => modifiers.shift = true,
                    "Alt" => modifiers.alt = true,
                    _ => return Err(err("unknown modifier")),
                }
            }
            Ok((key, modifiers))
        };
        match cue {
            "play" => cues.push((t, Cue::Play(true))),
            "stop" => cues.push((t, Cue::Play(false))),
            "key" => {
                let (k, m) = key(2)?;
                cues.push((t, Cue::KeyDown(k, m)));
                cues.push((t + num(3)?, Cue::KeyUp(k, m)));
            }
            "keydown" => {
                let (k, m) = key(2)?;
                cues.push((t, Cue::KeyDown(k, m)));
            }
            "keyup" => {
                let (k, m) = key(2)?;
                cues.push((t, Cue::KeyUp(k, m)));
            }
            "type" => {
                // Text typed a letter at a time, as a person would: a word, or
                // a "quoted phrase" with spaces
                let (text, gap) = match line.split_once('"') {
                    Some((_, rest)) => {
                        let (text, after) = rest.split_once('"').ok_or_else(|| err("unclosed quote"))?;
                        let gap = after.trim();
                        (text, if gap.is_empty() { 0.09 } else { gap.parse().map_err(|_| err("not a number"))? })
                    }
                    None => (*words.get(2).ok_or_else(|| err("missing text"))?, if words.len() > 3 { num(3)? } else { 0.09 }),
                };
                for (i, c) in text.chars().enumerate() {
                    cues.push((t + i as f64 * gap, Cue::Text(c.to_string())));
                }
            }
            "note" | "chord" => {
                let notes: Vec<u8> = words.get(2).ok_or_else(|| err("missing note"))?
                    .split(',')
                    .map(|n| n.parse().map_err(|_| err("bad note")))
                    .collect::<Result<_, _>>()?;
                let velocity = num(3)? as u8;
                let len = num(4)?;
                for note in notes {
                    cues.push((t, Cue::Note { note, velocity, on: true }));
                    cues.push((t + len, Cue::Note { note, velocity, on: false }));
                }
            }
            "midi" => {
                let event = match words.get(2).copied() {
                    Some("start") => MidiEvent::Start,
                    Some("stop") => MidiEvent::Stop,
                    Some("continue") => MidiEvent::Continue,
                    _ => return Err(err("expected start, stop or continue")),
                };
                cues.push((t, Cue::Midi(event)));
            }
            "midiclock" => {
                // A clock master's ticks, 24 to the beat
                let bpm = num(2)?;
                let len = num(3)?;
                if bpm <= 0.0 {
                    return Err(err("tempo must be positive"));
                }
                let spacing = 60.0 / (bpm * 24.0);
                let ticks = (len / spacing).floor() as usize;
                cues.extend((0..ticks).map(|n| (t + n as f64 * spacing, Cue::Midi(MidiEvent::Clock))));
            }
            "param" => {
                let target = words.get(2).ok_or_else(|| err("missing module"))?;
                let (module, which) = capture_aim::module_ref(target).map_err(err)?;
                // Input names may contain spaces: everything up to the value
                let value_at = (3..words.len()).find(|&i| words[i].parse::<f64>().is_ok()).ok_or_else(|| err("missing value"))?;
                let input = words[3..value_at].join(" ");
                cues.push((t, Cue::Param { module, which, input, value: num(value_at)? as f32, dur: opt(value_at + 1)? }));
            }
            "cursor" => cues.push((t, Cue::Cursor(words.get(2) != Some(&"off")))),
            "move" if words.get(2) == Some(&"off") && words.len() == 3 => cues.push((t, Cue::PointerOff)),
            "move" => {
                let (to, rest) = capture_aim::parse_spot(&words[2..]).map_err(err)?;
                if rest.len() > 1 {
                    return Err(err("too many values"));
                }
                cues.push((t, Cue::Move { to, dur: rest.first().copied().unwrap_or(0.0) }));
            }
            "press" => cues.push((t, Cue::Button(true))),
            "release" => cues.push((t, Cue::Button(false))),
            "rclick" => {
                cues.push((t, Cue::SecondaryButton(true)));
                cues.push((t + 0.05, Cue::SecondaryButton(false)));
            }
            "drag" => {
                let (to, rest) = capture_aim::parse_spot(&words[2..]).map_err(err)?;
                let &[dur] = rest.as_slice() else { return Err(err("expected one length")) };
                cues.push((t, Cue::Button(true)));
                cues.push((t + 0.05, Cue::Move { to, dur }));
                cues.push((t + 0.1 + dur, Cue::Button(false)));
            }
            "wheel" => cues.push((t, Cue::Wheel { delta: num(2)? as f32, ctrl: words.get(3) == Some(&"ctrl") })),
            "view" => cues.push((t, Cue::View { pan: Vec2::new(num(2)? as f32, num(3)? as f32), dur: opt(4)? })),
            "zoom" => cues.push((t, Cue::Zoom { factor: num(2)? as f32, dur: opt(3)? })),
            "camera" => cues.push((t, Cue::Camera {
                offset: Vec2::new(num(2)? as f32, num(3)? as f32),
                scale: num(4)? as f32,
                dur: opt(5)?,
            })),
            "fit" => cues.push((t, Cue::Fit(opt(2)?))),
            "aim" => cues.push((t, Cue::Aim { at: Pos2::new(num(2)? as f32, num(3)? as f32), zoom: num(4)? as f32, dur: opt(5)? })),
            "enter" => {
                // Names may have spaces: the rest of the line
                let name = line.splitn(3, char::is_whitespace).nth(2).map(str::trim).unwrap_or_default();
                // ...up to an aim on the end: `at x y zoom`
                let (name, aim) = match name.split_whitespace().collect::<Vec<_>>()[..] {
                    [ref group @ .., "at", x, y, zoom] if !group.is_empty() => {
                        let n = |s: &str| s.parse::<f32>().map_err(|_| err("bad number in the aim"));
                        (group.join(" "), Some((Pos2::new(n(x)?, n(y)?), n(zoom)?)))
                    }
                    _ => (name.to_string(), None),
                };
                if name.is_empty() {
                    return Err(err("missing group"));
                }
                cues.push((t, Cue::Enter(name, aim)));
            }
            "leave" => cues.push((t, Cue::Leave)),
            "still" => cues.push((t, Cue::Still(words.get(2).ok_or_else(|| err("missing name"))?.to_string()))),
            "layout" => cues.push((t, Cue::Layout(words.get(2).ok_or_else(|| err("missing name"))?.to_string()))),
            "input" => cues.push((t, Cue::Input(PathBuf::from(words.get(2).ok_or_else(|| err("missing file"))?)))),
            "hover" => {
                let file = words.get(2).ok_or_else(|| err("missing file"))?;
                // Whole, as a drag from the desktop carries it
                let path = (*file != "off").then(|| std::path::absolute(file).unwrap_or_else(|_| PathBuf::from(file)));
                cues.push((t, Cue::Hover(path)));
            }
            "drop" => cues.push((t, Cue::Drop)),
            "end" => cues.push((t, Cue::End)),
            _ => return Err(err("unknown cue")),
        }
    }
    // Stable: cues at the same time keep their script order
    cues.sort_by(|a, b| a.0.total_cmp(&b.0));
    if !cues.iter().any(|(_, c)| matches!(c, Cue::End)) {
        return Err("script has no `end` cue".into());
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_expands_lengths_and_sorts() {
        let cues = parse_script("1.0 key Z 0.5\n0.5 chord 60,64 90 1\n0.0 play\n3 param filter.svf#2 Cut Off 800 1\n4 end").unwrap();
        let times: Vec<f64> = cues.iter().map(|c| c.0).collect();
        assert_eq!(times, vec![0.0, 0.5, 0.5, 1.0, 1.5, 1.5, 1.5, 3.0, 4.0]);
        match &cues[7].1 {
            Cue::Param { module, which, input, value, dur } => {
                assert_eq!((module.as_str(), *which, input.as_str(), *value, *dur), ("filter.svf", Which::Nth(1), "Cut Off", 800.0, 1.0));
            }
            other => panic!("expected a param cue, got {:?}", other),
        }
    }

    #[test]
    fn script_plays_a_midi_clock() {
        let cues = parse_script("0.5 midi start\n0.5 midiclock 120 1.0\n2 midi stop\n3 end").unwrap();
        let ticks: Vec<f64> = cues.iter().filter(|c| matches!(c.1, Cue::Midi(MidiEvent::Clock))).map(|c| c.0).collect();
        // Two beats of ticks, 1/48 s apart
        assert_eq!(ticks.len(), 48);
        assert!((ticks[1] - ticks[0] - 1.0 / 48.0).abs() < 1e-12);
        assert!(matches!(cues[0].1, Cue::Midi(MidiEvent::Start)));
        assert!(parse_script("0 midi pause\n1 end").is_err());
    }

    #[test]
    fn fit_cuts_unless_given_a_length() {
        let cues = parse_script("0.0 fit\n1.0 fit 1.5\n2 end").unwrap();
        assert!(matches!(cues[0].1, Cue::Fit(d) if d == 0.0));
        assert!(matches!(cues[1].1, Cue::Fit(d) if d == 1.5));
    }

    #[test]
    fn the_pointer_can_leave_the_window() {
        let cues = parse_script("0 move 400 300 0.5
1 move off
2 end").unwrap();
        assert!(matches!(cues[1].1, Cue::PointerOff));
    }

    #[test]
    fn groups_are_entered_by_name_and_aimed_in_patch_space() {
        let cues = parse_script("0 enter Bells
1 enter Bells/FM Bell   # a path
2 leave
3 aim 1200 640.5 1.5 2
4 aim 0 0 1
5 end").unwrap();
        assert!(matches!(&cues[0].1, Cue::Enter(name, None) if name == "Bells"));
        assert!(matches!(&cues[1].1, Cue::Enter(name, None) if name == "Bells/FM Bell"));
        assert!(matches!(cues[2].1, Cue::Leave));
        assert!(matches!(cues[3].1, Cue::Aim { at, zoom, dur } if at == Pos2::new(1200.0, 640.5) && zoom == 1.5 && dur == 2.0));
        assert!(matches!(cues[4].1, Cue::Aim { dur, .. } if dur == 0.0));
        assert!(parse_script("0 enter
1 end").is_err());
        let cues = parse_script("0 enter Drums at 168 291.5 1.0
1 enter Bells/FM Bell at -20 40 2   # aimed inside a path
2 end").unwrap();
        assert!(matches!(&cues[0].1, Cue::Enter(name, Some((at, zoom))) if name == "Drums" && *at == Pos2::new(168.0, 291.5) && *zoom == 1.0));
        assert!(matches!(&cues[1].1, Cue::Enter(name, Some((at, _))) if name == "Bells/FM Bell" && *at == Pos2::new(-20.0, 40.0)));
        assert!(parse_script("0 enter Drums at 168 x 1
1 end").is_err());
        assert!(parse_script("0 aim 10 10
1 end").is_err());
    }

    #[test]
    fn a_hovered_file_is_held_whole_until_dropped_or_given_up() {
        let cues = parse_script("0 hover take.wav
1 drop
2 hover off
3 end").unwrap();
        assert!(matches!(&cues[0].1, Cue::Hover(Some(path)) if path.is_absolute() && path.ends_with("take.wav")));
        assert!(matches!(cues[1].1, Cue::Drop));
        assert!(matches!(cues[2].1, Cue::Hover(None)));
        assert!(parse_script("0 hover
1 end").is_err());
    }

    #[test]
    fn script_needs_an_end() {
        assert!(parse_script("0 play").is_err());
        assert!(parse_script("0 bogus\n1 end").is_err());
    }

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
    fn glide_eases_to_its_target() {
        let g = Glide { from: Vec2::ZERO, to: Vec2::new(10.0, 0.0), start: 1.0, dur: 2.0 };
        assert_eq!(g.at(0.0), Vec2::ZERO);
        assert_eq!(g.at(2.0), Vec2::new(5.0, 0.0));
        assert_eq!(g.at(5.0), Vec2::new(10.0, 0.0));
        assert!(g.done(3.0) && !g.done(2.9));
    }

    #[test]
    fn camera_glide_carries_the_centre_straight() {
        // From a close-up on a module at the patch's right edge (centre 5366
        // at zoom 1.4) out to the whole patch (centre 2962 at 0.2), as the
        // shots write them: offset = -(centre - c) * zoom
        let c = 960.0;
        let cam = |centre: f32, zoom: f32| (zoom, Vec2::new(-(centre - c) * zoom, 0.0));
        let g = CameraGlide { from: cam(5366.0, 1.4), to: cam(2962.0, 0.2), start: 0.0, dur: 1.0 };
        let centre = |t: f64| {
            let (zoom, offset) = g.at(t);
            c - offset.x / zoom
        };
        assert!((centre(0.0) - 5366.0).abs() < 0.01 && (centre(1.0) - 2962.0).abs() < 0.01);
        // Never past either end, and always moving the same way
        let mut last = centre(0.0);
        for i in 1..=50 {
            let x = centre(i as f64 / 50.0);
            assert!(x <= last + 1e-3 && x >= 2962.0 - 0.01, "centre {x} at step {i}");
            last = x;
        }
        assert!((g.at(0.5).0 - (1.4f32 * 0.2).sqrt()).abs() < 1e-4, "zoom eases in log space");
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
