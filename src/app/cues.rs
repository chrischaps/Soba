//! The cue language: what a capture script (and a presentation's beat)
//! says the app should do, and when. See `capture` for the script format;
//! this is its parser, shared so a script can be played by the
//! frame-stepped capture or live, on the wall clock (#487).

use std::path::PathBuf;

use eframe::egui;
use egui::{Pos2, Vec2};

use super::capture_aim::{self, PartAt, Spot, Which};
use crate::engine::MidiEvent;

/// How a script is read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    /// A whole capture script: it needs an `end`, and cues before 0 run
    /// during the warmup.
    Film,
    /// One beat of a presentation, timed from the beat's start: no `end`,
    /// nothing before 0, and nothing only a recording can do (`still`,
    /// `layout`, `input`).
    Beat,
}

/// A cue from the script.
#[derive(Debug, Clone)]
pub(crate) enum Cue {
    Play(bool),
    KeyDown(egui::Key, egui::Modifiers),
    KeyUp(egui::Key, egui::Modifiers),
    Text(String),
    /// A note on `channel` (0-15).
    Note { channel: u8, note: u8, velocity: u8, on: bool },
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
    /// A turn of the mouse wheel, in points, with Ctrl or Alt held or neither.
    Wheel { delta: f32, modifiers: egui::Modifiers },
    View { pan: Vec2, dur: f64 },
    Zoom { factor: f32, dur: f64 },
    Camera { offset: Vec2, scale: f32, dur: f64 },
    /// Frame the whole level, over this many seconds.
    Fit(f64),
    /// Centre a patch point at a zoom, over this many seconds.
    Aim { at: Pos2, zoom: f32, dur: f64 },
    /// Bring a module or frame into view, framed as F frames a selection or
    /// centred at a zoom, over this many seconds.
    Look { at: LookAt, zoom: Option<f32>, dur: f64 },
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
    /// Whatever is moving (a knob, the camera, the pointer) gets where it's
    /// going, now: a presenter finished the beat early (#489).
    Land,
    End,
}

impl Cue {
    /// The MIDI message a note or MIDI cue sends.
    pub fn midi_event(&self) -> Option<MidiEvent> {
        match *self {
            Cue::Note { channel, note, velocity, on: true } => Some(MidiEvent::NoteOn { channel, note, velocity }),
            Cue::Note { channel, note, on: false, .. } => Some(MidiEvent::NoteOff { channel, note, velocity: 0 }),
            Cue::Midi(event) => Some(event),
            _ => None,
        }
    }

    /// The cue as a script writes it, after its time: what a take (#489)
    /// writes of each cue a beat fired, to be read back the same. `None`
    /// for what isn't a line of its own: a note, whose length is its end's
    /// time (a take pairs the two up), a right-click's release (`rclick`
    /// lets go by itself), and a typed `"`, which a script can't quote.
    pub fn script(&self) -> Option<String> {
        let keys = |key: egui::Key, m: egui::Modifiers| {
            let held = [(m.command || m.ctrl, "Ctrl+"), (m.shift, "Shift+"), (m.alt, "Alt+")];
            held.iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect::<String>() + key.name()
        };
        let spot = |spot: &Spot| match spot {
            Spot::Point(p) => format!("{} {}", p.x, p.y),
            Spot::By(v) => format!("by {} {}", v.x, v.y),
            Spot::Part(part) => {
                let module = match part.frame() {
                    Some(_) => part.module.clone(),
                    None => format!("{}{}", part.module, part.which),
                };
                match &part.at {
                    PartAt::Named(name) if part.frame().is_some() && name == "title" => module,
                    PartAt::Named(name) => format!("{module} {name}"),
                    PartAt::Offset(v) => format!("{module} {} {}", v.x, v.y),
                }
            }
        };
        Some(match self {
            Cue::Play(true) => "play".into(),
            Cue::Play(false) => "stop".into(),
            Cue::KeyDown(key, m) => format!("keydown {}", keys(*key, *m)),
            Cue::KeyUp(key, m) => format!("keyup {}", keys(*key, *m)),
            Cue::Text(text) if text.contains('"') => return None,
            Cue::Text(text) => format!("type \"{text}\""),
            Cue::Note { .. } => return None,
            Cue::Midi(event) => match *event {
                MidiEvent::ControlChange { channel, controller, value } => format!("cc {} {controller} {value}", channel + 1),
                MidiEvent::PitchBend { channel, value } => format!("bend {} {value}", channel + 1),
                MidiEvent::ChannelPressure { channel, pressure } => format!("pressure {} {pressure}", channel + 1),
                MidiEvent::Start => "midi start".into(),
                MidiEvent::Stop => "midi stop".into(),
                MidiEvent::Continue => "midi continue".into(),
                MidiEvent::Clock => "midi clock".into(),
                // Nothing a script says sends anything else
                _ => return None,
            },
            Cue::Param { module, which, input, value, dur } => format!("param {module}{which} {input} {value} {dur}"),
            Cue::Cursor(on) => format!("cursor {}", if *on { "on" } else { "off" }),
            Cue::Move { to, dur } => format!("move {} {dur}", spot(to)),
            Cue::PointerOff => "move off".into(),
            Cue::Button(true) => "press".into(),
            Cue::Button(false) => "release".into(),
            Cue::SecondaryButton(true) => "rclick".into(),
            Cue::SecondaryButton(false) => return None,
            Cue::Wheel { delta, modifiers } => match (modifiers.ctrl || modifiers.command, modifiers.alt) {
                (true, _) => format!("wheel {delta} ctrl"),
                (false, true) => format!("wheel {delta} alt"),
                _ => format!("wheel {delta}"),
            },
            Cue::View { pan, dur } => format!("view {} {} {dur}", pan.x, pan.y),
            Cue::Zoom { factor, dur } => format!("zoom {factor} {dur}"),
            Cue::Camera { offset, scale, dur } => format!("camera {} {} {scale} {dur}", offset.x, offset.y),
            Cue::Fit(dur) => format!("fit {dur}"),
            Cue::Aim { at, zoom, dur } => format!("aim {} {} {zoom} {dur}", at.x, at.y),
            Cue::Look { at, zoom: Some(zoom), dur } => format!("look {at} {zoom} {dur}"),
            Cue::Look { at, zoom: None, dur } => format!("look {at} {dur}"),
            Cue::Enter(name, None) => format!("enter {name}"),
            Cue::Enter(name, Some((at, zoom))) => format!("enter {name} at {} {} {zoom}", at.x, at.y),
            Cue::Leave => "leave".into(),
            Cue::Still(name) => format!("still {name}"),
            Cue::Layout(name) => format!("layout {name}"),
            Cue::Input(path) => format!("input {}", path.display()),
            Cue::Hover(Some(path)) => format!("hover {}", path.display()),
            Cue::Hover(None) => "hover off".into(),
            Cue::Drop => "drop".into(),
            Cue::Land => "land".into(),
            Cue::End => "end".into(),
        })
    }
}

/// What a `look` cue brings into view.
#[derive(Debug, Clone, PartialEq)]
pub enum LookAt {
    /// A module, named as `param` names one.
    Module(String, Which),
    /// A frame, by its title.
    Frame(String),
}

impl std::fmt::Display for LookAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookAt::Module(module, which) => write!(f, "{module}{which}"),
            LookAt::Frame(title) => write!(f, "frame:{title}"),
        }
    }
}

/// A script's MIDI channel, 1-16, as the 0-15 a message carries.
fn midi_channel(n: f64) -> Option<u8> {
    (1.0..=16.0).contains(&n).then(|| n as u8 - 1)
}

/// Parses a script into time-ordered cues, expanding cues with a length
/// (key taps, notes, drags) into their start and end.
pub(crate) fn parse_cues(text: &str, mode: Mode) -> Result<Vec<(f64, Cue)>, String> {
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
        if mode == Mode::Beat {
            if t < 0.0 {
                return Err(err("a beat's cues are timed from its start, so none comes before 0"));
            }
            if matches!(cue, "still" | "layout" | "input" | "end") {
                return Err(err("only a capture can do this: a beat can't"));
            }
        }
        // A key, with any modifiers before it: `Z`, `Ctrl+Shift+F`
        let key = |i: usize| -> Result<(egui::Key, egui::Modifiers), String> {
            let name = words.get(i).ok_or_else(|| err("missing key"))?;
            let mut parts: Vec<&str> = name.split('+').collect();
            let key = egui::Key::from_name(parts.pop().unwrap_or_default()).ok_or_else(|| err("unknown key"))?;
            let mut modifiers = egui::Modifiers::default();
            for part in parts {
                match part {
                    "Ctrl" => modifiers |= egui::Modifiers::COMMAND,
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
                let channel = if words.len() > 5 { midi_channel(num(5)?).ok_or_else(|| err("channel is 1-16"))? } else { 0 };
                for note in notes {
                    cues.push((t, Cue::Note { channel, note, velocity, on: true }));
                    cues.push((t + len, Cue::Note { channel, note, velocity, on: false }));
                }
            }
            "cc" => {
                let channel = midi_channel(num(2)?).ok_or_else(|| err("channel is 1-16"))?;
                let byte = |i| match num(i)? {
                    n if (0.0..=127.0).contains(&n) => Ok(n as u8),
                    _ => Err(err("a CC's number and value are 0-127")),
                };
                cues.push((t, Cue::Midi(MidiEvent::ControlChange { channel, controller: byte(3)?, value: byte(4)? })));
            }
            "bend" => {
                let channel = midi_channel(num(2)?).ok_or_else(|| err("channel is 1-16"))?;
                let value = match num(3)? {
                    n if (-8192.0..=8191.0).contains(&n) => n as i16,
                    _ => return Err(err("a bend is -8192 to 8191")),
                };
                cues.push((t, Cue::Midi(MidiEvent::PitchBend { channel, value })));
            }
            "pressure" => {
                let channel = midi_channel(num(2)?).ok_or_else(|| err("channel is 1-16"))?;
                let pressure = match num(3)? {
                    n if (0.0..=127.0).contains(&n) => n as u8,
                    _ => return Err(err("pressure is 0-127")),
                };
                cues.push((t, Cue::Midi(MidiEvent::ChannelPressure { channel, pressure })));
            }
            "midi" => {
                let event = match words.get(2).copied() {
                    Some("start") => MidiEvent::Start,
                    Some("stop") => MidiEvent::Stop,
                    Some("continue") => MidiEvent::Continue,
                    // One tick, as a take writes a clock it heard
                    Some("clock") => MidiEvent::Clock,
                    _ => return Err(err("expected start, stop, continue or clock")),
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
            "wheel" => {
                let modifiers = match words.get(3) {
                    Some(&"ctrl") => egui::Modifiers::CTRL,
                    Some(&"alt") => egui::Modifiers::ALT,
                    _ => egui::Modifiers::default(),
                };
                cues.push((t, Cue::Wheel { delta: num(2)? as f32, modifiers }))
            }
            "view" => cues.push((t, Cue::View { pan: Vec2::new(num(2)? as f32, num(3)? as f32), dur: opt(4)? })),
            "zoom" => cues.push((t, Cue::Zoom { factor: num(2)? as f32, dur: opt(3)? })),
            "camera" => cues.push((t, Cue::Camera {
                offset: Vec2::new(num(2)? as f32, num(3)? as f32),
                scale: num(4)? as f32,
                dur: opt(5)?,
            })),
            "fit" => cues.push((t, Cue::Fit(opt(2)?))),
            "aim" => cues.push((t, Cue::Aim { at: Pos2::new(num(2)? as f32, num(3)? as f32), zoom: num(4)? as f32, dur: opt(5)? })),
            "look" => {
                let (at, numbers) = parse_look(&words[2..]).map_err(err)?;
                let (zoom, dur) = match numbers[..] {
                    [] => (None, 0.0),
                    [dur] => (None, dur),
                    [zoom, dur] if zoom > 0.0 => (Some(zoom as f32), dur),
                    [_, _] => return Err(err("a zoom is more than 0")),
                    _ => return Err(err("too many values: a look takes a length, or a zoom and a length")),
                };
                cues.push((t, Cue::Look { at, zoom, dur }));
            }
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
            "land" => cues.push((t, Cue::Land)),
            "end" => cues.push((t, Cue::End)),
            _ => return Err(err("unknown cue")),
        }
    }
    // Stable: cues at the same time keep their script order
    cues.sort_by(|a, b| a.0.total_cmp(&b.0));
    if mode == Mode::Film && !cues.iter().any(|(_, c)| matches!(c, Cue::End)) {
        return Err("script has no `end` cue".into());
    }
    Ok(cues)
}

/// Reads what a `look` cue brings into view, and the numbers after it: a
/// module (`filter.svf`, `osc.sine#2`, `seq.step@104`), or a frame by its
/// title, which runs to the numbers (`frame:Drive and echo`).
fn parse_look(words: &[&str]) -> Result<(LookAt, Vec<f64>), &'static str> {
    let first = *words.first().ok_or("missing module or frame")?;
    let number = |w: &&str| w.parse::<f64>().ok();
    let (at, rest) = match first.strip_prefix("frame:") {
        Some(title) => {
            let title_len = words[1..].iter().take_while(|w| number(w).is_none()).count();
            let title = std::iter::once(title).chain(words[1..=title_len].iter().copied()).collect::<Vec<_>>().join(" ");
            if title.is_empty() {
                return Err("missing frame title");
            }
            (LookAt::Frame(title), &words[1 + title_len..])
        }
        None => {
            let (module, which) = capture_aim::module_ref(first)?;
            if crate::graph::catalog::module(&module).is_none() {
                return Err("no module of that kind");
            }
            (LookAt::Module(module, which), &words[1..])
        }
    };
    let numbers = rest.iter().map(|w| number(w).ok_or("not a number")).collect::<Result<Vec<_>, _>>()?;
    Ok((at, numbers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn film(text: &str) -> Result<Vec<(f64, Cue)>, String> {
        parse_cues(text, Mode::Film)
    }

    #[test]
    fn script_expands_lengths_and_sorts() {
        let cues = film("1.0 key Z 0.5\n0.5 chord 60,64 90 1\n0.0 play\n3 param filter.svf#2 Cut Off 800 1\n4 end").unwrap();
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
        let cues = film("0.5 midi start\n0.5 midiclock 120 1.0\n2 midi stop\n3 end").unwrap();
        let ticks: Vec<f64> = cues.iter().filter(|c| matches!(c.1, Cue::Midi(MidiEvent::Clock))).map(|c| c.0).collect();
        // Two beats of ticks, 1/48 s apart
        assert_eq!(ticks.len(), 48);
        assert!((ticks[1] - ticks[0] - 1.0 / 48.0).abs() < 1e-12);
        assert!(matches!(cues[0].1, Cue::Midi(MidiEvent::Start)));
        assert!(film("0 midi pause\n1 end").is_err());
    }

    #[test]
    fn script_hits_a_pad_on_its_channel_and_moves_a_cc() {
        let cues = film("0.5 note 36 110 0.1 10\n1 cc 1 74 127\n2 end").unwrap();
        assert!(matches!(cues[0].1, Cue::Note { channel: 9, note: 36, velocity: 110, on: true }));
        assert!(matches!(cues[1].1, Cue::Note { channel: 9, on: false, .. }));
        assert!(matches!(cues[2].1, Cue::Midi(MidiEvent::ControlChange { channel: 0, controller: 74, value: 127 })));
        assert!(matches!(cues[0].1.midi_event(), Some(MidiEvent::NoteOn { channel: 9, note: 36, velocity: 110 })));
        assert!(matches!(cues[1].1.midi_event(), Some(MidiEvent::NoteOff { channel: 9, note: 36, velocity: 0 })));
        assert!(film("0 note 60 100 1\n1 end").is_ok_and(|cues| matches!(cues[0].1, Cue::Note { channel: 0, .. })));
        assert!(film("0 cc 17 1 1\n1 end").is_err());
        assert!(film("0 cc 1 1 128\n1 end").is_err());
        let cues = film("0 bend 2 -8192\n0 pressure 1 90\n1 end").unwrap();
        assert!(matches!(cues[0].1, Cue::Midi(MidiEvent::PitchBend { channel: 1, value: -8192 })));
        assert!(matches!(cues[1].1, Cue::Midi(MidiEvent::ChannelPressure { channel: 0, pressure: 90 })));
        assert!(film("0 bend 1 9000\n1 end").is_err());
    }

    #[test]
    fn fit_cuts_unless_given_a_length() {
        let cues = film("0.0 fit\n1.0 fit 1.5\n2 end").unwrap();
        assert!(matches!(cues[0].1, Cue::Fit(d) if d == 0.0));
        assert!(matches!(cues[1].1, Cue::Fit(d) if d == 1.5));
    }

    #[test]
    fn the_pointer_can_leave_the_window() {
        let cues = film("0 move 400 300 0.5
1 move off
2 end").unwrap();
        assert!(matches!(cues[1].1, Cue::PointerOff));
    }

    #[test]
    fn groups_are_entered_by_name_and_aimed_in_patch_space() {
        let cues = film("0 enter Bells
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
        assert!(film("0 enter
1 end").is_err());
        let cues = film("0 enter Drums at 168 291.5 1.0
1 enter Bells/FM Bell at -20 40 2   # aimed inside a path
2 end").unwrap();
        assert!(matches!(&cues[0].1, Cue::Enter(name, Some((at, zoom))) if name == "Drums" && *at == Pos2::new(168.0, 291.5) && *zoom == 1.0));
        assert!(matches!(&cues[1].1, Cue::Enter(name, Some((at, _))) if name == "Bells/FM Bell" && *at == Pos2::new(-20.0, 40.0)));
        assert!(film("0 enter Drums at 168 x 1
1 end").is_err());
        assert!(film("0 aim 10 10
1 end").is_err());
    }

    #[test]
    fn a_look_frames_a_module_or_frame_by_name() {
        let cues = film("0 look filter.svf
1 look osc.sine#2 1.2
2 look seq.step@104 1.4 2
3 look frame:Drive and echo 0.8
4 end").unwrap();
        let look = |i: usize| match &cues[i].1 {
            Cue::Look { at, zoom, dur } => (at.clone(), *zoom, *dur),
            other => panic!("expected a look, got {other:?}"),
        };
        assert_eq!(look(0), (LookAt::Module("filter.svf".into(), Which::Nth(0)), None, 0.0));
        assert_eq!(look(1), (LookAt::Module("osc.sine".into(), Which::Nth(1)), None, 1.2));
        assert_eq!(look(2), (LookAt::Module("seq.step".into(), Which::Id(104)), Some(1.4), 2.0));
        assert_eq!(look(3), (LookAt::Frame("Drive and echo".into()), None, 0.8));
        assert!(film("0 look\n1 end").is_err());
        assert!(film("0 look osc.nope 1\n1 end").is_err(), "a module kind that doesn't exist fails as the script loads");
        assert!(film("0 look osc.sine 1 2 3\n1 end").is_err());
        assert!(film("0 look osc.sine 0 2\n1 end").is_err());
        assert!(film("0 look frame: 1\n1 end").is_err());
    }

    #[test]
    fn a_hovered_file_is_held_whole_until_dropped_or_given_up() {
        let cues = film("0 hover take.wav
1 drop
2 hover off
3 end").unwrap();
        assert!(matches!(&cues[0].1, Cue::Hover(Some(path)) if path.is_absolute() && path.ends_with("take.wav")));
        assert!(matches!(cues[1].1, Cue::Drop));
        assert!(matches!(cues[2].1, Cue::Hover(None)));
        assert!(film("0 hover
1 end").is_err());
    }

    #[test]
    fn script_needs_an_end() {
        assert!(film("0 play").is_err());
        assert!(film("0 bogus\n1 end").is_err());
    }

    #[test]
    fn a_beat_has_no_end_and_nothing_only_a_recording_does() {
        let cues = parse_cues("0 play\n0.5 param filter.svf Cutoff 800 2\n1 note 60 100 0.5", Mode::Beat).unwrap();
        assert_eq!(cues.len(), 4);
        for cue in ["0 still x", "0 layout x", "0 input take.wav", "1 end"] {
            assert!(parse_cues(cue, Mode::Beat).is_err(), "{cue}");
            assert!(parse_cues(&format!("{cue}\n2 end"), Mode::Film).is_ok(), "{cue}");
        }
        assert!(parse_cues("-0.5 play", Mode::Beat).is_err(), "a beat starts at its start");
        assert!(parse_cues("", Mode::Beat).is_ok_and(|cues| cues.is_empty()));
    }

    #[test]
    fn a_cue_written_out_reads_back_the_same() {
        // Every cue a beat can fire, as a take writes them (#489)
        let script = "0 play
0 stop
0 keydown Ctrl+Shift+F
0 keyup Z
0 type \"a b\"
0 cc 2 64 127
0 bend 1 -8192
0 pressure 3 90
0 midi start
0 midi clock
0 param filter.svf#2 Cut Off 800.5 1.5
0 param seq.step@104 Rate 0.25 0
0 cursor off
0 move 400 300 0.6
0 move by 0 -60 2
0 move osc.sine#2 knob:Detune 0.3
0 move fx.eq 234 135 0
0 move frame:Drive and echo 0.3
0 move frame:Shape 10 20 0
0 move off
0 press
0 release
0 rclick
0 wheel -120
0 wheel 240 ctrl
0 wheel 120 alt
0 view 120 -40 2
0 zoom 1.4 2
0 camera -300 0 0.5 3
0 fit 1.5
0 aim 1200 640 1.5 2
0 look filter.svf 1.2
0 look osc.sine#2 1.4 2
0 look frame:Drive and echo 1
0 enter Bells/FM Bell
0 enter Drums at 168 291 1
0 leave
0 hover off
0 drop
0 land
1 end";
        let cues = film(script).unwrap();
        let written: String = cues.iter().filter_map(|(at, cue)| Some(format!("{at} {}
", cue.script()?))).collect();
        let again = film(&written).unwrap();
        let debug = |cues: &[(f64, Cue)]| cues.iter().filter(|(_, c)| !matches!(c, Cue::SecondaryButton(false))).map(|c| format!("{c:?}")).collect::<Vec<_>>();
        assert_eq!(debug(&again), debug(&cues), "{written}");
        assert_eq!(Cue::Text("\"".into()).script(), None, "a quote can't be typed in a script");
    }
}
