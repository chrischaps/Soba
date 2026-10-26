//! Presentation mode (#488): a script, written ahead of time, played one
//! beat at a time, live. Each press of Page Down (what a presentation
//! clicker sends) fires the next beat: a few capture cues, timed from the
//! beat's own start, that move the camera, turn knobs and play notes while
//! the presenter talks over them.
//!
//! ```text
//! title    How Synth Filters Work
//! advance  cc 64                    # a footswitch on CC 64 plays the next beat
//! back     cc 65 1                  # ...and one on CC 65, channel 1 only, goes back
//!
//! beat Resonance
//! say   Now turn up resonance until the filter starts to sing.
//! 0     look filter.svf 1.4 1.2
//! 0     cursor on
//! 0     move knob:Resonance 0.8     # (see `capture` for every cue)
//! 1.0   param filter.svf Resonance 0.95 4.0
//! auto  6                           # on to the next beat 6 s in, unpressed
//! ```
//!
//! `say` is what the presenter says, shown large in the notes window, one
//! paragraph a line, with `**bold**` for the words to lean on. A beat's
//! cues are anything a capture script can do but record (`still`, `layout`,
//! `input`, `end`).
//!
//! A footswitch (#489) is a CC named before the first beat, for `advance`,
//! `back` or `finish`: any USB pedal or controller then steps the beats.
//! While presenting, the MIDI engine keeps it from everything else, so
//! nothing in the patch hears it (CC 64 is the sustain pedal), nor any
//! thru route or mapping.
//!
//! Each run is kept as a take (see `presentation_take`), a capture script
//! that films it again exactly.
//!
//! This is the script and its stepping; `synth_app/present.rs` plays the
//! cues into the app and draws the notes.

use std::collections::VecDeque;

use eframe::egui;
use egui::Pos2;

use super::capture_aim::{self, Part, Spot};
use super::cue_runner::{CaptureAction, CueRunner};
use super::cues::{parse_cues, Cue, Mode};
use super::presentation_take::Take;

/// Within this long of a beat's start, going back goes to the beat before,
/// as a music player's back button does; after it, the beat starts again.
pub const BACK_WINDOW: f64 = 1.0;

/// The shortest a camera move or zoom takes, live: a frame at 60 Hz.
const STEP: f64 = 1.0 / 60.0;

/// A presentation's script.
#[derive(Debug, Clone)]
pub struct Script {
    pub title: Option<String>,
    /// The footswitches that step it.
    pub pedals: Vec<Pedal>,
    pub beats: Vec<Beat>,
}

/// A MIDI footswitch the presenter steps the beats with: a CC, on one
/// channel or any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pedal {
    pub command: Command,
    pub controller: u8,
    /// 0-15, or `None` for any.
    pub channel: Option<u8>,
}

impl Pedal {
    /// Whether a CC on `channel` (0-15) is this pedal's.
    pub fn hears(&self, channel: u8, controller: u8) -> bool {
        controller == self.controller && self.channel.is_none_or(|c| c == channel)
    }

    /// The pedal's CC, and its channel if only one.
    pub fn name(&self) -> String {
        let channel = self.channel.map(|c| format!(" on channel {}", c + 1)).unwrap_or_default();
        format!("CC {}{channel}", self.controller)
    }
}

/// One beat: its cues, and what's said over them.
#[derive(Debug, Clone)]
pub struct Beat {
    pub name: String,
    /// What the presenter says, a paragraph a line.
    pub say: Vec<String>,
    /// Seconds after it starts that the next beat follows on its own.
    pub auto: Option<f64>,
    pub(crate) cues: Vec<(f64, Cue)>,
}

impl Beat {
    /// Whether the beat's cues take the pointer or keys over: presses,
    /// drags, keys, typing, the wheel and dropped files. Moves alone only
    /// draw the script's pointer, and leave the real one free.
    fn takes_over(&self) -> bool {
        self.cues.iter().any(|(_, cue)| {
            matches!(
                cue,
                Cue::Button(_)
                    | Cue::SecondaryButton(_)
                    | Cue::Wheel { .. }
                    | Cue::KeyDown(..)
                    | Cue::KeyUp(..)
                    | Cue::Text(_)
                    | Cue::Hover(_)
                    | Cue::Drop
            )
        })
    }
}

/// Reads a presentation script. Errors name the line.
pub fn parse(text: &str) -> Result<Script, String> {
    let mut title = None;
    let mut pedals: Vec<Pedal> = Vec::new();
    // Each beat, and the lines of its cues
    let mut beats: Vec<(Beat, Vec<usize>)> = Vec::new();
    for (line_no, raw) in text.lines().enumerate() {
        // `#` starts a comment, as in a capture script
        let line = match raw.find(" #") {
            Some(at) => &raw[..at],
            None if raw.trim_start().starts_with('#') => "",
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let err = |msg: &str| format!("script line {}: {} ({})", line_no + 1, msg, line);
        let (word, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(w, r)| (w, r.trim()));
        if word == "beat" {
            let name = if rest.is_empty() { format!("Beat {}", beats.len() + 1) } else { rest.to_string() };
            beats.push((Beat { name, say: Vec::new(), auto: None, cues: Vec::new() }, Vec::new()));
            continue;
        }
        if word == "title" {
            if !beats.is_empty() {
                return Err(err("the title comes before the first beat"));
            }
            title = Some(rest.to_string()).filter(|t| !t.is_empty());
            continue;
        }
        let command = match word {
            "advance" => Some(Command::Next),
            "back" => Some(Command::Back),
            "finish" => Some(Command::Finish),
            _ => None,
        };
        if let Some(command) = command {
            if !beats.is_empty() {
                return Err(err("footswitches are named before the first beat"));
            }
            let pedal = parse_pedal(command, rest).map_err(err)?;
            let shared = |p: &&Pedal| p.controller == pedal.controller && (p.channel.is_none() || pedal.channel.is_none() || p.channel == pedal.channel);
            if let Some(other) = pedals.iter().find(shared) {
                return Err(err(&format!("{} steps the beats already", other.name())));
            }
            pedals.push(pedal);
            continue;
        }
        let Some((beat, lines)) = beats.last_mut() else {
            return Err(err("everything but the title belongs to a beat: start one with `beat <name>`"));
        };
        match word {
            "say" => beat.say.push(rest.to_string()),
            "auto" => {
                let seconds = rest.parse::<f64>().ok().filter(|s| *s > 0.0).ok_or_else(|| err("auto takes the seconds until the next beat"))?;
                if beat.auto.replace(seconds).is_some() {
                    return Err(err("a beat has one auto"));
                }
            }
            _ if word.parse::<f64>().is_ok() => lines.push(line_no),
            _ => return Err(err("expected title, advance, back, finish, beat, say, auto or a cue")),
        }
    }
    if beats.is_empty() {
        return Err("the script has no beats: start one with `beat <name>`".into());
    }
    let lines: Vec<&str> = text.lines().collect();
    let beats = beats
        .into_iter()
        .map(|(mut beat, mine)| {
            // The beat's cue lines where they are, the rest blank, so a
            // cue's error names its line in the file
            let text = (0..lines.len()).map(|i| if mine.contains(&i) { lines[i] } else { "" }).collect::<Vec<_>>().join("\n");
            beat.cues = parse_cues(&text, Mode::Beat)?;
            for (at, cue) in &beat.cues {
                if let Cue::Move { to: Spot::Part(part), .. } = cue {
                    capture_aim::check(part).map_err(|e| format!("beat {}: the move at {at:.2} s: {e}", beat.name))?;
                }
            }
            Ok(beat)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Script { title, pedals, beats })
}

/// A footswitch's line after its command: `cc <number> [channel]`.
fn parse_pedal(command: Command, rest: &str) -> Result<Pedal, &'static str> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let [kind, number, channel @ ..] = words.as_slice() else { return Err("a footswitch is `cc <number>`, and a channel if only one") };
    if *kind != "cc" {
        return Err("a footswitch is a CC: `cc <number>`");
    }
    // 120-127 are the channel mode messages, All Notes Off among them
    let controller = number.parse::<u8>().ok().filter(|n| *n < 120).ok_or("a footswitch's CC is 0-119")?;
    let channel = match channel {
        [] => None,
        [n] => Some(n.parse::<u8>().ok().filter(|n| (1..=16).contains(n)).ok_or("the channel is 1-16")? - 1),
        _ => return Err("too many values: `cc <number> [channel]`"),
    };
    Ok(Pedal { command, controller, channel })
}

/// What the presenter asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// The next beat. One still playing finishes first.
    Next,
    /// The beat on show again, from the moment it began; or, pressed just
    /// as it began, the one before.
    Back,
    /// The beat on show ends now, as it would have.
    Finish,
}

/// A presentation under way: which beat is on show, and its cues playing.
pub struct Presentation {
    pub script: Script,
    runner: CueRunner,
    /// The beat on show, or `None` before the first.
    beat: Option<usize>,
    /// When it began, on the app's clock.
    started: f64,
    /// The app's clock as of the last pass.
    now: f64,
    /// The runner's clock as of the last pass.
    last: f64,
    /// Its `auto` has asked for the next beat.
    auto_done: bool,
    /// The beat has the pointer and keys.
    taken_over: bool,
    commands: VecDeque<Command>,
    /// A beat to start once the one on show has landed.
    start_next: Option<usize>,
    /// The runner as each beat began, to play it again from.
    began_with: Vec<Option<CueRunner>>,
    /// A beat began this pass: the app keeps how things stood.
    began: Option<usize>,
    /// A beat begins again this pass: the app puts things back as they stood.
    restore: Option<usize>,
    /// The beat waits a pass before its cues fire: things put back have
    /// to be drawn before a camera move can set out from them.
    hold: bool,
    /// The run so far, as a capture script would film it.
    take: Take,
}

impl Presentation {
    pub fn new(script: Script) -> Self {
        let beats = script.beats.len();
        let mut runner = CueRunner::new(Vec::new(), STEP);
        runner.keep_log();
        Self {
            script,
            runner,
            beat: None,
            started: 0.0,
            now: 0.0,
            last: 0.0,
            auto_done: false,
            taken_over: false,
            commands: VecDeque::new(),
            start_next: None,
            began_with: vec![None; beats],
            began: None,
            restore: None,
            hold: false,
            take: Take::default(),
        }
    }

    /// The beat on show, if one has begun.
    pub fn beat(&self) -> Option<usize> {
        self.beat
    }

    /// How long the beat on show has been on, at `now`.
    pub fn elapsed(&self, now: f64) -> f64 {
        if self.beat.is_some() { (now - self.started).max(0.0) } else { 0.0 }
    }

    /// Whether the beat on show still has cues to play or moves to make.
    pub fn playing(&self) -> bool {
        self.beat.is_some() && !self.runner.done()
    }

    /// Seconds until the beat's `auto` moves on, at `now`, and the whole wait.
    pub fn auto_left(&self, now: f64) -> Option<(f64, f64)> {
        let k = self.beat?;
        let auto = self.script.beats[k].auto?;
        (!self.auto_done && k + 1 < self.script.beats.len()).then(|| ((auto - self.elapsed(now)).max(0.0), auto))
    }

    /// Whether the app has to keep drawing: a beat playing, an `auto` to
    /// wait out, or a press waiting for its pass.
    pub fn busy(&self) -> bool {
        self.playing() || !self.commands.is_empty() || self.start_next.is_some() || self.auto_left(self.now).is_some()
    }

    /// Asks for `command`, which is obeyed as the next pass begins.
    pub fn command(&mut self, command: Command) {
        self.commands.push_back(command);
    }

    /// The runner's clock: the beat's time.
    fn clock(&self) -> f64 {
        self.now - self.started
    }

    /// Obeys what was asked, then fires the beat's cues due by now. Called
    /// before each pass with its input, `now` its time. A beat that takes
    /// over has its cues' input in place of the real pointer's; any other
    /// leaves the input alone. `locate` finds a module's part on screen.
    pub fn prepare_input(&mut self, now: f64, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        self.step(now, raw, locate);
        // What fired, into the take
        let fired = self.runner.take_log();
        if !fired.is_empty() {
            self.take.keep(self.started, fired);
        }
    }

    fn step(&mut self, now: f64, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        self.now = now;
        if let Some((left, _)) = self.auto_left(now) {
            if left <= 0.0 && self.commands.is_empty() && self.start_next.is_none() {
                self.auto_done = true;
                self.commands.push_back(Command::Next);
            }
        }
        if self.start_next.is_some() && self.runner.done() {
            let k = self.start_next.take().unwrap_or_default();
            self.start(k, false);
        } else if self.start_next.is_none() {
            if let Some(command) = self.commands.pop_front() {
                self.obey(command, raw, locate);
            }
        }
        if self.beat.is_none() {
            return;
        }
        if std::mem::take(&mut self.hold) {
            self.started = now;
            if let Some(k) = self.beat {
                self.take.again(k, now);
            }
            return;
        }

        let t = self.clock();
        let mut scratch = egui::RawInput::default();
        let input = if self.taken_over {
            // The script's pointer, not the hand's
            raw.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerGone
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Touch { .. }
                        | egui::Event::Zoom(_)
                )
            });
            &mut *raw
        } else {
            &mut scratch
        };
        self.runner.fire(t + 1e-6, input, locate, &mut |_, _| None);
        self.runner.follow_aim(locate);
        if !self.runner.modifiers().is_none() {
            input.events.push(egui::Event::ModifiersChanged(self.runner.modifiers()));
        }
        self.runner.animate(self.last, t, input);
        self.last = t;
        // Done with the pointer: the hand has it back
        if self.taken_over && self.runner.done() {
            self.taken_over = false;
            raw.events.push(egui::Event::PointerGone);
        }
    }

    fn obey(&mut self, command: Command, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        let beats = self.script.beats.len();
        match command {
            Command::Next => {
                let next = self.beat.map_or(0, |k| k + 1);
                if self.playing() {
                    self.land(raw, locate);
                    if next < beats {
                        self.start_next = Some(next);
                    }
                } else if next < beats {
                    self.start(next, false);
                }
            }
            Command::Finish => {
                if self.playing() {
                    self.land(raw, locate);
                }
            }
            Command::Back => {
                let Some(k) = self.beat else { return };
                let to = if self.elapsed(self.now) < BACK_WINDOW && k > 0 && self.began_with[k - 1].is_some() { k - 1 } else { k };
                let Some(runner) = self.began_with[to].clone() else { return };
                self.let_go(raw);
                self.runner = runner;
                self.start(to, true);
            }
        }
    }

    /// Finishes the beat on show at once.
    fn land(&mut self, raw: &mut egui::RawInput, locate: &mut dyn FnMut(&Part) -> Result<Pos2, String>) {
        let mut scratch = egui::RawInput::default();
        let input = if self.taken_over { &mut *raw } else { &mut scratch };
        self.runner.finish(self.clock(), input, locate);
    }

    /// Lets go of whatever the beat on show holds, before it's dropped.
    fn let_go(&mut self, raw: &mut egui::RawInput) {
        if self.taken_over {
            let mut finished = self.runner.clone();
            finished.finish(self.clock(), raw, &mut |_| Err(String::new()));
            raw.events.push(egui::Event::PointerGone);
        }
    }

    /// Begins beat `k` now; `again` puts things back as they stood when it
    /// first began.
    fn start(&mut self, k: usize, again: bool) {
        if again {
            self.restore = Some(k);
            self.hold = true;
        } else {
            self.began_with[k] = Some(self.runner.clone());
            self.began = Some(k);
            self.take.begin(k, self.now);
        }
        let beat = &self.script.beats[k];
        self.runner.load(beat.cues.clone());
        self.taken_over = beat.takes_over();
        self.beat = Some(k);
        self.started = self.now;
        self.last = 0.0;
        self.auto_done = false;
        self.start_next = None;
    }

    /// The run so far as a capture script, ending now; `None` before the
    /// first press. `patch` is the patch's file.
    pub fn take_script(&self, patch: Option<&str>) -> Option<String> {
        self.take.script(&self.script, patch, self.now)
    }

    /// How long the take has run at `now`, once the first beat has begun.
    pub fn take_length(&self, now: f64) -> Option<f64> {
        self.take.length(now)
    }

    /// Whether the take waits for how things stood at the first press.
    pub fn take_wants_opening(&self) -> bool {
        self.take.started() && !self.take.opened()
    }

    /// How things stood at the first press, as cues.
    pub fn open_take(&mut self, cues: Vec<String>) {
        self.take.open(cues);
    }

    /// A beat that began this pass, for the app to note how things stand.
    pub fn take_began(&mut self) -> Option<usize> {
        self.began.take()
    }

    /// A beat begun again this pass, for the app to put things back.
    pub fn take_restore(&mut self) -> Option<usize> {
        self.restore.take()
    }

    /// Whether a cue couldn't find what it aimed at.
    pub fn failed(&self) -> bool {
        self.runner.failed()
    }

    /// The app's work for this pass.
    pub fn take_actions(&mut self) -> Vec<CaptureAction> {
        self.runner.take_actions()
    }

    /// The knobs the beat is turning, where they are now.
    pub fn param_values(&mut self, current: impl FnMut(&str, capture_aim::Which, &str) -> Option<f32>) -> Vec<CaptureAction> {
        let t = self.clock();
        self.runner.param_values(t, current)
    }

    /// The zoom to apply this pass (1.0 for none).
    pub fn take_zoom(&mut self) -> f32 {
        self.runner.take_zoom()
    }

    /// The pan the graph should have, given its current pan.
    pub fn take_pan(&mut self, current: egui::Vec2) -> Option<egui::Vec2> {
        let t = self.clock();
        self.runner.take_pan(t, current)
    }

    /// Draws the script's pointer.
    pub fn draw_cursor(&self, ctx: &egui::Context) {
        self.runner.draw_cursor(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "title  How Synth Filters Work

beat Resonance
say   Now turn up resonance until the filter starts to sing.
say   Hear it **whistle**?
0     look filter.svf 1.4 1.2
0     cursor on
1.0   param filter.svf Resonance 0.95 4.0
auto  6

beat   # named for its place
0     play
0.5   note 60 100 0.5
";

    #[test]
    fn a_script_is_beats_of_cues_and_words() {
        let script = parse(SCRIPT).unwrap();
        assert_eq!(script.title.as_deref(), Some("How Synth Filters Work"));
        assert_eq!(script.beats.len(), 2);
        let first = &script.beats[0];
        assert_eq!(first.name, "Resonance");
        assert_eq!(first.say, ["Now turn up resonance until the filter starts to sing.", "Hear it **whistle**?"]);
        assert_eq!(first.auto, Some(6.0));
        assert_eq!(first.cues.len(), 3);
        assert!(!first.takes_over(), "a look, the cursor and a knob leave the pointer free");
        let second = &script.beats[1];
        assert_eq!(second.name, "Beat 2");
        assert_eq!((second.cues.len(), second.auto), (3, None));
    }

    #[test]
    fn the_shipped_presentations_read() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("presentations");
        let mut read = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "txt") {
                let text = std::fs::read_to_string(&path).unwrap();
                parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                read += 1;
            }
        }
        assert!(read > 0, "no scripts in {}", dir.display());
    }

    #[test]
    fn a_bad_line_says_where_it_is() {
        let err = |text: &str| parse(text).unwrap_err();
        assert!(err("beat A\n0 play\n\n1 bogus").starts_with("script line 4:"), "{}", err("beat A\n0 play\n\n1 bogus"));
        assert!(err("0 play").contains("belongs to a beat"));
        assert!(err("say hello").contains("belongs to a beat"));
        assert!(err("beat A\n1 still x").starts_with("script line 2:"));
        assert!(err("beat A\nauto 0").contains("auto"));
        assert!(err("beat A\nauto 2\nauto 3").contains("one auto"));
        assert!(err("beat A\ntitle late").contains("before the first beat"));
        assert!(err("title Nothing").contains("no beats"));
        assert!(err("beat A\nnarrate hi").contains("expected title"));
        assert!(err("beat A\n0 move filter.svf knob:Nope 1").contains("beat A"));
    }

    #[test]
    fn presses_and_keys_take_the_pointer_over() {
        let script = parse("beat A\n0 move 10 10 0.5\nbeat B\n0 drag 40 40 0.5\nbeat C\n0 type osc").unwrap();
        let over: Vec<bool> = script.beats.iter().map(Beat::takes_over).collect();
        assert_eq!(over, [false, true, true]);
    }

    /// A pass at `now`, with no input but the runner's; what the app is
    /// asked to do, by name.
    fn pass(p: &mut Presentation, now: f64) -> Vec<String> {
        p.prepare_input(now, &mut egui::RawInput::default(), &mut |_| Err("isn't drawn".into()));
        let mut done: Vec<String> = p.take_actions().iter().map(|a| match a {
            CaptureAction::Play(on) => format!("play {on}"),
            CaptureAction::Midi { event, .. } => format!("{event:?}").split_whitespace().next().unwrap_or_default().to_string(),
            CaptureAction::Look { seconds, .. } => format!("look {seconds}"),
            other => format!("{other:?}"),
        }).collect();
        done.extend(p.param_values(|_, _, _| Some(0.0)).iter().map(|a| match a {
            CaptureAction::SetParam { value, .. } => format!("knob {value:.2}"),
            other => format!("{other:?}"),
        }));
        done
    }

    #[test]
    fn each_press_plays_the_next_beat_from_its_start() {
        let mut p = Presentation::new(parse("beat A\n0 play\n1 look filter.svf 1.4 2\nbeat B\n0.5 note 60 100 0.25").unwrap());
        assert!(pass(&mut p, 0.0).is_empty(), "nothing before the first press");
        assert_eq!(p.beat(), None);
        p.command(Command::Next);
        assert_eq!(pass(&mut p, 3.0), ["play true"]);
        assert_eq!(p.take_began(), Some(0));
        assert_eq!(pass(&mut p, 4.1), ["look 2"]);
        assert!(!p.playing());
        p.command(Command::Next);
        assert!(pass(&mut p, 10.0).is_empty());
        assert_eq!((p.beat(), p.take_began()), (Some(1), Some(1)));
        assert_eq!(pass(&mut p, 10.6), ["NoteOn"]);
        assert_eq!(pass(&mut p, 10.8), ["NoteOff"]);
        // Past the last beat, a press does nothing
        p.command(Command::Next);
        assert!(pass(&mut p, 12.0).is_empty());
        assert_eq!(p.beat(), Some(1));
    }

    #[test]
    fn a_press_mid_beat_lands_it_then_moves_on() {
        let mut p = Presentation::new(parse("beat A\n0 note 60 100 4\n1 look filter.svf 1.4 2\n2 param filter.svf Cutoff 800 3\nbeat B\n0 stop").unwrap());
        p.command(Command::Next);
        assert_eq!(pass(&mut p, 0.0), ["NoteOn"]);
        p.command(Command::Next);
        // The rest of A at once: the camera cuts, the note's let go, the
        // knob lands; B waits for the pass after
        assert_eq!(pass(&mut p, 0.5), ["look 0", "NoteOff", "knob 800.00"]);
        assert_eq!(p.beat(), Some(0));
        assert_eq!(pass(&mut p, 0.52), ["play false"]);
        assert_eq!(p.beat(), Some(1));
    }

    #[test]
    fn finish_lands_the_beat_and_stays() {
        let mut p = Presentation::new(parse("beat A\n0 param filter.svf Cutoff 800 3\n2 note 60 100 1\nbeat B\n0 stop").unwrap());
        p.command(Command::Next);
        assert_eq!(pass(&mut p, 0.0), ["knob 0.00"]);
        p.command(Command::Finish);
        assert_eq!(pass(&mut p, 1.0), ["knob 800.00"], "a note not yet played isn't");
        assert_eq!((p.beat(), p.playing()), (Some(0), false));
    }

    #[test]
    fn back_plays_the_beat_again_or_the_one_before() {
        let mut p = Presentation::new(parse("beat A\n0 play\nbeat B\n0.5 stop").unwrap());
        p.command(Command::Next);
        pass(&mut p, 0.0);
        p.command(Command::Next);
        pass(&mut p, 5.0);
        assert_eq!(p.take_began(), Some(1));
        assert_eq!(pass(&mut p, 5.6), ["play false"]);
        // Well into B: B again, as it stood
        p.command(Command::Back);
        assert!(pass(&mut p, 8.0).is_empty());
        assert_eq!((p.beat(), p.take_restore(), p.take_began()), (Some(1), Some(1), None));
        // That pass is for things put back to be drawn; the beat plays on from it
        assert_eq!(pass(&mut p, 8.6), ["play false"]);
        p.command(Command::Back);
        pass(&mut p, 9.5);
        assert_eq!((p.beat(), p.take_restore()), (Some(1), Some(1)), "1.5 s in is B again");
        // Just after B began again: back to A, from the pass after
        p.command(Command::Back);
        assert!(pass(&mut p, 9.6).is_empty());
        assert_eq!((p.beat(), p.take_restore()), (Some(0), Some(0)));
        assert_eq!(pass(&mut p, 9.62), ["play true"]);
    }

    #[test]
    fn auto_moves_on_by_itself() {
        let mut p = Presentation::new(parse("beat A\n0 play\nauto 2\nbeat B\n0 stop").unwrap());
        p.command(Command::Next);
        pass(&mut p, 1.0);
        assert_eq!(p.auto_left(2.5), Some((0.5, 2.0)));
        assert!(pass(&mut p, 2.9).is_empty());
        assert_eq!(pass(&mut p, 3.0), ["play false"]);
        assert_eq!(p.beat(), Some(1));
        assert_eq!(p.auto_left(3.0), None, "the last beat has nowhere to go");
    }

    #[test]
    fn footswitches_are_named_before_the_beats() {
        let script = parse("advance cc 64\nback  cc 65 2\nbeat A\n0 play").unwrap();
        assert_eq!(
            script.pedals,
            [
                Pedal { command: Command::Next, controller: 64, channel: None },
                Pedal { command: Command::Back, controller: 65, channel: Some(1) },
            ]
        );
        assert!(script.pedals[1].hears(1, 65) && !script.pedals[1].hears(0, 65) && script.pedals[0].hears(9, 64));
        let err = |text: &str| parse(text).unwrap_err();
        assert!(err("beat A\nadvance cc 64").contains("before the first beat"));
        assert!(err("advance note 64\nbeat A").contains("a CC"));
        assert!(err("advance cc 64\nback cc 64 1\nbeat A").contains("CC 64 steps the beats already"));
        assert!(err("advance cc 123\nbeat A").contains("0-119"));
        assert!(err("finish cc 66 17\nbeat A").contains("1-16"));
        assert!(err("advance cc\nbeat A").starts_with("script line 1:"));
    }

    #[test]
    fn the_take_is_the_run_as_it_went() {
        let mut p = Presentation::new(parse("beat A\n0 play\n1 param filter.svf Cutoff 800 4\n3 look filter.svf 1.4 2\nbeat B\n0 note 60 100 1").unwrap());
        assert_eq!(p.take_script(None), None, "nothing before the first press");
        p.command(Command::Next);
        pass(&mut p, 10.0);
        pass(&mut p, 11.5);
        // On to B 2 s into A: what A had left lands, as a film of it will
        p.command(Command::Next);
        pass(&mut p, 12.0);
        pass(&mut p, 12.02);
        assert_eq!(p.beat(), Some(1));
        // B again: its first go isn't kept
        p.command(Command::Back);
        pass(&mut p, 14.0);
        pass(&mut p, 14.5);
        pass(&mut p, 15.2);
        assert!(p.take_length(15.2).is_some_and(|s| (s - 5.2).abs() < 1e-9));
        let text = p.take_script(Some("patches/filters.json")).unwrap();
        let film = parse_cues(&text, Mode::Film).unwrap_or_else(|e| panic!("{e}\n{text}"));
        let fired: Vec<String> = film.iter().map(|(at, cue)| format!("{at} {}", cue.script().unwrap_or_else(|| format!("{cue:?}")))).collect();
        assert_eq!(
            fired,
            [
                "0 play",
                "1 param filter.svf#1 Cutoff 800 4",
                "2 look filter.svf#1 1.4 0",
                "2 land",
                "4 Note { channel: 0, note: 60, velocity: 100, on: true }",
                "5 Note { channel: 0, note: 60, velocity: 100, on: false }",
                "5.7 end",
            ],
            "{text}"
        );
    }
}
