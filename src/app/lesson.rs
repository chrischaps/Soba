//! Lessons (#305): short tutorials inside the app, which you do rather than
//! read. A caption sits beside the part it's about, that part glows, a ghost
//! pointer shows the move once, and the lesson waits until you've made it
//! yourself.
//!
//! A lesson is a text file in `lessons/`, one line at a time. It reads from
//! the top down rather than on a clock, as a capture script does: each
//! `wait` closes a step, and the lesson holds there until the patch shows
//! the step is done.
//!
//! ```text
//! # A comment
//! title First Sound
//! about An oscillator, an output, and a cable between them
//! patch shaping-with-a-filter.json        # what the lesson opens: one of
//!                                          # lessons/*.json, or none for an
//!                                          # empty canvas
//! say Drag from the Oscillator's **Out** to the output's **Mono**.
//! say A second `say` is another paragraph; `Z` is drawn as a key.
//! point osc.sine out:Out                   # make a part glow; the caption
//! point output.audio in:Mono               # sits beside the first
//! point canvas                             # ...or some empty canvas
//! point group                              # ...or the first group on show
//! point filter.svf                         # ...or a whole module
//! show drag osc.sine out:Out to output.audio in:Mono
//! show rclick canvas                       # also move, click, dclick
//! show turn filter.svf knob:Cutoff down    # a knob dragged down (or up)
//! wait connect osc.sine Out -> output.audio Mono
//! ```
//!
//! Parts are named as a capture names them (see `capture_aim`). The ghost's
//! move plays once as its step starts, and again on *Show me*.
//!
//! The waits look at the patch, not at how it got that way, so a cable
//! dragged either way counts, and a module added from the menu or the
//! palette:
//!
//! ```text
//! wait next                                # a Next button
//! wait add osc.sine                        # the patch has an oscillator
//! wait add osc.sine#2                      # ...two of them
//! wait connect osc.sine Out -> filter.svf In   # a cable, through any groups
//! wait param filter.svf Cutoff < 400       # a knob below, above (>) or at (=)
//! wait param osc.sine Waveform = Saw       # a dropdown, by its label
//! wait param seq.step Step * Pitch changed # moved from where it was: any
//!                                          # step's, with a `*`
//! wait play                                # the transport is running
//! wait listen 4                            # four seconds of it
//! wait notes 3                             # three notes played, from the
//!                                          # computer keyboard, a piano or MIDI
//! wait probe 1.5                           # a cable heard with Alt held
//! wait group                               # the patch has a group
//! wait enter / wait leave                  # inside a group, or back out
//! ```
//!
//! A module named by kind alone is any module of that kind; `#n` or `@id`
//! names one. A wait that's already met when the lesson reaches it passes
//! at once, so doing a step early is fine. Whatever comes after the last
//! wait is said as the lesson ends.

use std::collections::HashMap;

use eframe::egui;
use egui_node_graph2::{InputId, NodeId};

use super::capture_aim::{self, Part, Which};
use crate::dsp::ParameterDisplay;
use crate::graph::groups::{self, NodeKind};
use crate::graph::{catalog, SynthGraph, SynthGraphState, SynthValueType};

/// A lesson that ships with the app: compiled in, so Help → Lessons works
/// wherever the app runs, the browser too.
#[derive(Debug)]
pub struct LessonFile {
    /// Its file in `lessons/`, which `?lesson=` and `--lesson` name without
    /// the `.txt`.
    pub file_name: &'static str,
    pub text: &'static str,
}

impl LessonFile {
    /// The name `?lesson=` and `--lesson` use: the file's, without `.txt`.
    pub fn key(&self) -> &'static str {
        self.file_name.trim_end_matches(".txt")
    }

    /// The lesson, read. The ones that ship are checked by the tests.
    pub fn lesson(&self) -> Result<Lesson, String> {
        Lesson::parse(self.text)
    }
}

/// Every lesson, in the order they're taken.
pub const LESSONS: &[LessonFile] = &[
    LessonFile { file_name: "first-sound.txt", text: include_str!("../../lessons/first-sound.txt") },
    LessonFile { file_name: "shaping-with-a-filter.txt", text: include_str!("../../lessons/shaping-with-a-filter.txt") },
    LessonFile { file_name: "envelopes.txt", text: include_str!("../../lessons/envelopes.txt") },
    LessonFile { file_name: "your-first-sequence.txt", text: include_str!("../../lessons/your-first-sequence.txt") },
    LessonFile { file_name: "groups.txt", text: include_str!("../../lessons/groups.txt") },
];

/// The patches lessons start from, by their file name in `lessons/`.
pub const LESSON_PATCHES: &[(&str, &str)] = &[
    ("shaping-with-a-filter.json", include_str!("../../lessons/shaping-with-a-filter.json")),
    ("envelopes.json", include_str!("../../lessons/envelopes.json")),
    ("your-first-sequence.json", include_str!("../../lessons/your-first-sequence.json")),
    ("groups.json", include_str!("../../lessons/groups.json")),
];

/// The lesson `?lesson=` or `--lesson` names: `first-sound`, with or
/// without `.txt`.
pub fn find(name: &str) -> Option<&'static LessonFile> {
    let name = name.trim_end_matches(".txt");
    LESSONS.iter().find(|l| l.key() == name)
}

/// The lesson after this one, if there is one.
pub fn after(file: &LessonFile) -> Option<&'static LessonFile> {
    let at = LESSONS.iter().position(|l| l.file_name == file.file_name)?;
    LESSONS.get(at + 1)
}

/// The patch a lesson starts from, by its file name.
pub fn patch_json(file_name: &str) -> Option<&'static str> {
    LESSON_PATCHES.iter().find(|(name, _)| *name == file_name).map(|(_, json)| *json)
}

/// A lesson, read from its file.
#[derive(Debug, Clone, PartialEq)]
pub struct Lesson {
    pub title: String,
    /// One line on what it teaches, for the menu.
    pub about: String,
    /// The patch in `lessons/` it opens, or `None` for an empty canvas.
    pub patch: Option<String>,
    pub steps: Vec<Step>,
}

/// One step: what's said and shown, and what it waits for. The step after
/// the last wait, if there is one, has no wait: it's said as the lesson ends.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Step {
    /// Paragraphs, with `**bold**` and `` `keys` ``.
    pub say: Vec<String>,
    /// Parts that glow; the caption sits beside the first that's on show.
    pub points: Vec<Spot>,
    pub show: Option<Show>,
    pub wait: Option<Wait>,
}

/// Somewhere on screen a lesson points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Spot {
    Part(Part),
    /// A stretch of empty canvas, for a right-click.
    Canvas,
    /// The first group on show.
    Group,
    /// A whole module.
    Module(String, Which),
}

/// The ghost pointer's move.
#[derive(Debug, Clone, PartialEq)]
pub enum Show {
    Move(Spot),
    Click(Spot),
    RightClick(Spot),
    DoubleClick(Spot),
    Drag(Spot, Spot),
    /// A knob dragged up (true) or down.
    Turn(Part, bool),
}

impl Show {
    /// Where the move goes, in order.
    pub fn spots(&self) -> Vec<Spot> {
        match self {
            Show::Move(s) | Show::Click(s) | Show::RightClick(s) | Show::DoubleClick(s) => vec![s.clone()],
            Show::Drag(a, b) => vec![a.clone(), b.clone()],
            Show::Turn(p, _) => vec![Spot::Part(p.clone())],
        }
    }
}

/// A module a wait names: any of a kind, or one of them.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRef {
    pub kind: String,
    /// `None` for any module of the kind.
    pub which: Option<Which>,
}

/// One end of a cable.
#[derive(Debug, Clone, PartialEq)]
pub struct Jack {
    pub module: ModuleRef,
    pub port: String,
}

/// What a step waits for.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    /// The Next button.
    Next,
    /// At least this many modules of a kind.
    Add { kind: String, count: usize },
    /// A cable from an output to an input, through any groups.
    Connect { from: Jack, to: Jack },
    Param { module: ModuleRef, input: String, test: Test },
    Play,
    /// Seconds of the transport running.
    Listen(f64),
    /// Notes played.
    Notes(usize),
    /// Seconds of a cable heard through the probe.
    Probe(f64),
    Group,
    Enter,
    Leave,
}

/// What a parameter's value is waited for.
#[derive(Debug, Clone, PartialEq)]
pub enum Test {
    Above(f32),
    Below(f32),
    Is(Value),
    Changed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f32),
    /// A dropdown's choice, by its label.
    Label(String),
}

impl Lesson {
    /// Reads a lesson, checking every part, module and port it names
    /// against what modules have, so a misspelt lesson fails its test.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lesson = Lesson { title: String::new(), about: String::new(), patch: None, steps: Vec::new() };
        let mut step = Step::default();
        for (line_no, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |msg: &str| format!("lesson line {}: {} ({})", line_no + 1, msg, line);
            let (cue, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
            let words: Vec<&str> = rest.split_whitespace().collect();
            match cue {
                "title" => lesson.title = rest.to_string(),
                "about" => lesson.about = rest.to_string(),
                "patch" => lesson.patch = Some(rest.to_string()).filter(|p| !p.is_empty()),
                "say" if !rest.is_empty() => step.say.push(rest.to_string()),
                "point" => step.points.push(spot(&words).map_err(|e| err(&e))?),
                "show" => step.show = Some(show(&words).map_err(|e| err(&e))?),
                "wait" => {
                    step.wait = Some(wait(rest).map_err(|e| err(&e))?);
                    lesson.steps.push(std::mem::take(&mut step));
                }
                _ => return Err(err("unknown line")),
            }
        }
        if step != Step::default() {
            lesson.steps.push(step);
        }
        if lesson.title.is_empty() {
            return Err("a lesson needs a title".into());
        }
        if !lesson.steps.iter().any(|s| s.wait.is_some()) {
            return Err(format!("{} waits for nothing", lesson.title));
        }
        Ok(lesson)
    }

    /// How many steps wait for something: the count the caption shows.
    pub fn waits(&self) -> usize {
        self.steps.iter().filter(|s| s.wait.is_some()).count()
    }
}

/// `canvas`, or a module's part.
fn spot(words: &[&str]) -> Result<Spot, String> {
    match words {
        ["canvas"] => return Ok(Spot::Canvas),
        ["group"] => return Ok(Spot::Group),
        [word] if !word.starts_with("frame:") => {
            let (kind, which) = capture_aim::module_ref(word)?;
            catalog::module(&kind).ok_or_else(|| format!("no module kind {kind}"))?;
            return Ok(Spot::Module(kind, which));
        }
        _ => {}
    }
    Ok(Spot::Part(part(words)?))
}

/// A module's (or frame's) part, as a capture names it, with nothing after.
fn part(words: &[&str]) -> Result<Part, String> {
    match capture_aim::parse_spot(words)? {
        (capture_aim::Spot::Part(part), rest) if rest.is_empty() => {
            capture_aim::check(&part)?;
            Ok(part)
        }
        (capture_aim::Spot::Part(_), _) => Err("too many numbers".into()),
        _ => Err("a lesson points at a module's part or the canvas, not the window".into()),
    }
}

fn show(words: &[&str]) -> Result<Show, String> {
    let (&kind, rest) = words.split_first().ok_or("missing move")?;
    match kind {
        "move" => Ok(Show::Move(spot(rest)?)),
        "click" => Ok(Show::Click(spot(rest)?)),
        "rclick" => Ok(Show::RightClick(spot(rest)?)),
        "dclick" => Ok(Show::DoubleClick(spot(rest)?)),
        "drag" => {
            let to = rest.iter().position(|w| *w == "to").ok_or("a drag goes from one spot to another")?;
            Ok(Show::Drag(spot(&rest[..to])?, spot(&rest[to + 1..])?))
        }
        "turn" => {
            let (&way, part_words) = rest.split_last().ok_or("missing knob")?;
            let up = match way {
                "up" => true,
                "down" => false,
                _ => return Err("a knob turns up or down".into()),
            };
            Ok(Show::Turn(part(part_words)?, up))
        }
        _ => Err("expected move, click, rclick, dclick, drag or turn".into()),
    }
}

/// A module named by kind (any of them), `kind#n` or `kind@id`.
fn module(word: &str) -> Result<ModuleRef, String> {
    let (kind, which) = capture_aim::module_ref(word)?;
    catalog::module(&kind).ok_or_else(|| format!("no module kind {kind}"))?;
    let named = word.contains('#') || word.contains('@');
    Ok(ModuleRef { kind, which: named.then_some(which) })
}

fn wait(rest: &str) -> Result<Wait, String> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (&kind, args) = words.split_first().ok_or("missing what to wait for")?;
    let seconds = |default: f64| -> Result<f64, String> {
        match args {
            [] => Ok(default),
            [s] => s.parse().map_err(|_| "not a number".to_string()),
            _ => Err("too many values".into()),
        }
    };
    let bare = |w: Wait| if args.is_empty() { Ok(w) } else { Err(format!("`wait {kind}` takes nothing more")) };
    match kind {
        "next" => bare(Wait::Next),
        "play" => bare(Wait::Play),
        "group" => bare(Wait::Group),
        "enter" => bare(Wait::Enter),
        "leave" => bare(Wait::Leave),
        "listen" => Ok(Wait::Listen(seconds(3.0)?)),
        "probe" => Ok(Wait::Probe(seconds(1.0)?)),
        "notes" | "note" => Ok(Wait::Notes(seconds(1.0)? as usize)),
        "add" => {
            let [word] = args else { return Err("`wait add` names one module".into()) };
            let (kind, which) = capture_aim::module_ref(word)?;
            catalog::module(&kind).ok_or_else(|| format!("no module kind {kind}"))?;
            let count = match which {
                Which::Nth(n) => n + 1,
                Which::Id(_) => return Err("`wait add` counts modules: kind#n, not @id".into()),
            };
            Ok(Wait::Add { kind, count })
        }
        "connect" => {
            // `osc.sine Out -> filter.svf In`: port names may have spaces
            let (from, to) = rest[kind.len()..].split_once("->").ok_or("a cable is `from -> to`")?;
            let jack = |text: &str, output: bool| -> Result<Jack, String> {
                let words: Vec<&str> = text.split_whitespace().collect();
                let (&first, port) = words.split_first().ok_or("missing module")?;
                let module = module(first)?;
                let port = port.join(" ");
                let port = port.strip_prefix(if output { "out:" } else { "in:" }).unwrap_or(&port).to_string();
                let spec = catalog::module(&module.kind).expect("checked by module()");
                let is = |name: &str| name.eq_ignore_ascii_case(&port);
                let found = if output {
                    spec.outputs().any(|p| is(p.name))
                } else {
                    spec.inputs().any(|p| is(p.name)) || spec.parameters.iter().any(|p| is(p.name))
                };
                if !found {
                    return Err(format!("{} has no {} {port}", module.kind, if output { "output" } else { "input" }));
                }
                Ok(Jack { module, port })
            };
            Ok(Wait::Connect { from: jack(from, true)?, to: jack(to, false)? })
        }
        "param" => {
            let (&first, rest) = args.split_first().ok_or("missing module")?;
            let module = module(first)?;
            let op = rest.iter().position(|w| matches!(*w, "<" | ">" | "=" | "changed")).ok_or("expected <, >, = or changed")?;
            let input = rest[..op].join(" ");
            let spec = catalog::module(&module.kind).expect("checked by module()");
            let param = spec
                .parameters
                .iter()
                .find(|p| matches(&input, p.name))
                .ok_or_else(|| format!("{} has no parameter {input}", module.kind))?;
            let value = rest[op + 1..].join(" ");
            let number = || value.parse::<f32>().map_err(|_| format!("{value} isn't a number"));
            let test = match rest[op] {
                "changed" if value.is_empty() => Test::Changed,
                "changed" => return Err("`changed` takes no value".into()),
                "<" => Test::Below(number()?),
                ">" => Test::Above(number()?),
                _ => match (&param.display, value.parse::<f32>()) {
                    (_, Ok(n)) => Test::Is(Value::Number(n)),
                    (ParameterDisplay::Discrete { labels }, Err(_)) => {
                        if !labels.iter().any(|l| l.eq_ignore_ascii_case(&value)) {
                            return Err(format!("{input} has no choice {value}; it has {}", labels.join(", ")));
                        }
                        Test::Is(Value::Label(value))
                    }
                    _ => return Err(format!("{value} isn't a number")),
                },
            };
            // A pattern stays one; a name is spelt as the module spells it
            let input = if input.contains('*') { input } else { param.name.to_string() };
            Ok(Wait::Param { module, input, test })
        }
        _ => Err("unknown wait".into()),
    }
}

/// What a wait looks at: the patch, and what's being played.
pub struct Facts<'a> {
    pub graph: &'a SynthGraph,
    pub state: &'a SynthGraphState,
    pub playing: bool,
    /// Notes held, from any keyboard.
    pub notes_held: usize,
    /// Whether a cable is being heard through the probe.
    pub probing: bool,
}

impl Facts<'_> {
    /// The modules a reference means.
    fn modules(&self, module: &ModuleRef) -> Vec<NodeId> {
        match module.which {
            Some(which) => capture_aim::find_module(self.graph, self.state, &module.kind, which).into_iter().collect(),
            None => self.graph.nodes.iter().filter(|(_, n)| n.user_data.module_id == module.kind).map(|(id, _)| id).collect(),
        }
    }

    /// A module's inputs that a name (or `*` pattern) means, and their values.
    fn params<'g>(&'g self, node: NodeId, input: &'g str) -> impl Iterator<Item = (InputId, &'g SynthValueType)> + 'g {
        let inputs = self.graph.nodes.get(node).map_or(&[][..], |n| &n.inputs[..]);
        inputs.iter().filter(move |(name, _)| matches(input, name)).map(|(_, id)| (*id, &self.graph.get_input(*id).value))
    }
}

/// Whether the port `id` is the one a node's list names `port`.
fn named<Id: PartialEq>(ports: &[(String, Id)], id: Id, port: &str) -> bool {
    ports.iter().any(|(name, p)| *p == id && name.eq_ignore_ascii_case(port))
}

/// Whether a parameter's name is the one a lesson means: the same but for
/// case, or with a `*` standing for any run of characters (`Step * Pitch`).
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        Some((head, tail)) => {
            let (name, head, tail) = (name.to_lowercase(), head.to_lowercase(), tail.to_lowercase());
            name.len() >= head.len() + tail.len() && name.starts_with(&head) && name.ends_with(&tail)
        }
        None => pattern.eq_ignore_ascii_case(name),
    }
}

/// How far a wait has got: what it measures from when the lesson reached it.
#[derive(Debug, Default)]
pub struct Progress {
    seconds: f64,
    notes: usize,
    /// Notes held last frame, once it's looked.
    held: Option<usize>,
    /// Whether `baseline` has been taken.
    began: bool,
    /// Each value as the wait began, for `changed`.
    baseline: HashMap<InputId, f32>,
}

impl Wait {
    /// Whether the wait is met, `dt` seconds since it was last asked.
    pub fn met(&self, facts: &Facts, progress: &mut Progress, dt: f64) -> bool {
        // Notes are counted as they're played, whatever's being waited for
        let held = facts.notes_held;
        progress.notes += held.saturating_sub(progress.held.unwrap_or(held));
        progress.held = Some(held);
        match self {
            Wait::Next => false,
            Wait::Add { kind, count } => facts.graph.nodes.values().filter(|n| n.user_data.module_id == *kind).count() >= *count,
            Wait::Connect { from, to } => {
                let (sources, sinks) = (facts.modules(&from.module), facts.modules(&to.module));
                groups::leaf_cables(facts.graph).into_iter().any(|(output, input)| {
                    let (o, i) = (facts.graph.get_output(output).node, facts.graph.get_input(input).node);
                    sources.contains(&o)
                        && sinks.contains(&i)
                        && named(&facts.graph.nodes[o].outputs, output, &from.port)
                        && named(&facts.graph.nodes[i].inputs, input, &to.port)
                })
            }
            Wait::Param { module, input, test } => {
                let nodes = facts.modules(module);
                if !progress.began {
                    progress.began = true;
                    for &node in &nodes {
                        progress.baseline.extend(facts.params(node, input).map(|(id, value)| (id, value.actual_value())));
                    }
                }
                nodes.iter().flat_map(|&node| facts.params(node, input)).any(|(id, value)| {
                    let v = value.actual_value();
                    match test {
                        Test::Above(x) => v > *x,
                        Test::Below(x) => v < *x,
                        Test::Is(Value::Number(x)) => (v - x).abs() < 1e-3,
                        Test::Is(Value::Label(label)) => match value {
                            SynthValueType::Select { value, options, .. } => {
                                options.get(*value).is_some_and(|o| o.eq_ignore_ascii_case(label))
                            }
                            _ => false,
                        },
                        // A module added since the wait began hasn't been
                        // moved yet: it's measured from where it starts
                        Test::Changed => (v - *progress.baseline.entry(id).or_insert(v)).abs() > 1e-4,
                    }
                })
            }
            Wait::Play => facts.playing,
            Wait::Listen(seconds) => {
                if facts.playing {
                    progress.seconds += dt;
                }
                progress.seconds >= *seconds
            }
            Wait::Notes(count) => progress.notes >= *count,
            Wait::Probe(seconds) => {
                if facts.probing {
                    progress.seconds += dt;
                }
                progress.seconds >= *seconds
            }
            Wait::Group => facts.graph.nodes.values().any(|n| matches!(n.user_data.kind, NodeKind::Group(_))),
            Wait::Enter => facts.state.level.is_some(),
            Wait::Leave => facts.state.level.is_none(),
        }
    }

    /// How far along a timed wait is, from 0 to 1, for the caption to show.
    pub fn fraction(&self, progress: &Progress) -> Option<f32> {
        let of = |done: f64, whole: f64| Some((done / whole.max(1e-9)).clamp(0.0, 1.0) as f32);
        match self {
            Wait::Listen(s) | Wait::Probe(s) => of(progress.seconds, *s),
            Wait::Notes(n) if *n > 1 => of(progress.notes as f64, *n as f64),
            _ => None,
        }
    }
}

/// A lesson being taken.
pub struct LessonRun {
    pub file: &'static LessonFile,
    pub lesson: Lesson,
    /// The step on show.
    pub step: usize,
    pub progress: Progress,
    /// When the step's wait was met, for its tick to show before it moves on.
    pub met_at: Option<f64>,
    /// When the step began, and when its ghost last started.
    pub began: Option<f64>,
    pub ghost_from: Option<f64>,
    /// When the lesson last looked, for the timed waits.
    pub last_time: Option<f64>,
    /// The empty canvas the step points at, found as it began.
    pub canvas: Option<egui::Pos2>,
}

impl LessonRun {
    pub fn new(file: &'static LessonFile, lesson: Lesson) -> Self {
        Self { file, lesson, step: 0, progress: Progress::default(), met_at: None, began: None, ghost_from: None, last_time: None, canvas: None }
    }

    pub fn current(&self) -> Option<&Step> {
        self.lesson.steps.get(self.step)
    }

    /// The number of the step on show, counting the steps that wait.
    pub fn number(&self) -> usize {
        self.lesson.steps[..self.step.min(self.lesson.steps.len())].iter().filter(|s| s.wait.is_some()).count() + 1
    }

    /// Whether every wait is met: what's left is the closing words.
    pub fn finished(&self) -> bool {
        self.current().is_none_or(|s| s.wait.is_none())
    }

    /// On to the next step.
    pub fn advance(&mut self) {
        self.step += 1;
        self.progress = Progress::default();
        self.met_at = None;
        self.began = None;
        self.ghost_from = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{graph_io::stage_patch, patch_from_json};

    #[test]
    fn every_lesson_reads_and_opens_its_patch() {
        assert_eq!(LESSONS.len(), 5);
        for file in LESSONS {
            let lesson = file.lesson().unwrap_or_else(|e| panic!("{}: {e}", file.file_name));
            assert!(!lesson.about.is_empty(), "{} needs an about line for the menu", file.file_name);
            if let Some(patch) = &lesson.patch {
                let json = patch_json(patch).unwrap_or_else(|| panic!("{}: {patch} isn't in LESSON_PATCHES", file.file_name));
                stage_patch(&patch_from_json(json).unwrap()).unwrap();
            }
            for step in &lesson.steps {
                assert!(!step.say.is_empty() || step.wait == Some(Wait::Play), "{}: a step with nothing to say", file.file_name);
            }
        }
        assert_eq!(find("first-sound").map(|l| l.file_name), Some("first-sound.txt"));
        assert_eq!(find("groups.txt").map(|l| l.file_name), Some("groups.txt"));
        assert!(find("nope").is_none());
        assert_eq!(after(&LESSONS[0]).map(|l| l.file_name), Some("shaping-with-a-filter.txt"));
        assert!(after(LESSONS.last().unwrap()).is_none());
    }

    #[test]
    fn a_lesson_is_steps_closed_by_waits() {
        let lesson = Lesson::parse(
            "# comment
title Test
about What it's for
say One
say Two
point osc.sine out:Out
show drag osc.sine out:Out to output.audio in:Mono
wait connect osc.sine Out -> output.audio in:Mono
point canvas
show rclick canvas
say Add a filter
wait add filter.svf#2
show turn filter.svf knob:Cutoff down
say Turn it
wait param filter.svf#1 Cutoff < 400
say Well done",
        )
        .unwrap();
        assert_eq!((lesson.title.as_str(), lesson.about.as_str(), lesson.patch.as_deref()), ("Test", "What it's for", None));
        assert_eq!(lesson.steps.len(), 4);
        assert_eq!(lesson.waits(), 3);
        assert_eq!(lesson.steps[0].say, ["One", "Two"]);
        assert!(matches!(&lesson.steps[0].show, Some(Show::Drag(Spot::Part(_), Spot::Part(_)))));
        let Some(Wait::Connect { from, to }) = &lesson.steps[0].wait else { panic!() };
        assert_eq!((from.module.kind.as_str(), from.module.which, from.port.as_str()), ("osc.sine", None, "Out"));
        assert_eq!(to.port, "Mono", "in: is dropped");
        assert_eq!(lesson.steps[1].points, [Spot::Canvas]);
        assert_eq!(lesson.steps[1].wait, Some(Wait::Add { kind: "filter.svf".into(), count: 2 }));
        assert!(matches!(&lesson.steps[2].show, Some(Show::Turn(_, false))));
        let Some(Wait::Param { module, input, test }) = &lesson.steps[2].wait else { panic!() };
        assert_eq!((module.which, input.as_str(), test), (Some(Which::Nth(0)), "Cutoff", &Test::Below(400.0)));
        assert_eq!(lesson.steps[3].wait, None, "the closing words");
    }

    #[test]
    fn a_misspelt_lesson_fails_as_it_loads() {
        let bad = |line: &str| Lesson::parse(&format!("title T\n{line}\nwait next")).unwrap_err();
        assert!(bad("wait connect osc.sine Outt -> output.audio Mono").contains("no output Outt"));
        assert!(bad("wait connect osc.sine Out output.audio Mono").contains("->"));
        assert!(bad("wait param osc.sine Waveform = Sawtooth").contains("Saw"));
        assert!(bad("wait param osc.sine Wobble > 1").contains("no parameter"));
        assert!(bad("wait add osc.nope").contains("no module kind"));
        assert!(bad("point osc.sine knob:Nope").contains("knob:Detune"));
        assert!(bad("point 400 300").contains("window"));
        assert!(bad("show turn filter.svf knob:Cutoff sideways").contains("up or down"));
        assert!(bad("show drag osc.sine out:Out").contains("from one spot to another"));
        assert!(bad("wait play now").contains("takes nothing"));
        assert!(bad("shout hello").contains("unknown line"));
        assert!(Lesson::parse("say no title\nwait next").is_err());
        assert!(Lesson::parse("title Nothing to do\nsay hi").is_err());
        let ok = Lesson::parse("title T\nwait param osc.sine Waveform = saw\nwait param seq.step Step 3 Pitch changed").unwrap();
        assert_eq!(ok.steps[0].wait, Some(Wait::Param {
            module: ModuleRef { kind: "osc.sine".into(), which: None },
            input: "Waveform".into(),
            test: Test::Is(Value::Label("saw".into())),
        }));
        assert!(matches!(&ok.steps[1].wait, Some(Wait::Param { input, test: Test::Changed, .. }) if input == "Step 3 Pitch"));
        let any = Lesson::parse("title T
wait param seq.step step * pitch changed").unwrap();
        assert!(matches!(&any.steps[0].wait, Some(Wait::Param { input, .. }) if input == "step * pitch"), "a pattern stays one");
        assert!(Lesson::parse("title T
wait param seq.step Step * Wobble changed").is_err());
    }

    /// The editor graph of a patch, as the app would hold it.
    fn staged(json: &str) -> (SynthGraph, SynthGraphState) {
        let staged = stage_patch(&patch_from_json(json).unwrap()).unwrap();
        (staged.graph, SynthGraphState::default())
    }

    fn check(wait: &str, graph: &SynthGraph, state: &SynthGraphState) -> bool {
        let lesson = Lesson::parse(&format!("title T\nwait {wait}")).unwrap();
        let facts = Facts { graph, state, playing: false, notes_held: 0, probing: false };
        lesson.steps[0].wait.as_ref().unwrap().met(&facts, &mut Progress::default(), 0.0)
    }

    #[test]
    fn waits_read_the_patch() {
        let (graph, state) = staged(include_str!("../../patches/first-sound.json"));
        let yes = |w: &str| assert!(check(w, &graph, &state), "{w}");
        let no = |w: &str| assert!(!check(w, &graph, &state), "{w}");
        yes("add osc.sine");
        no("add osc.sine#2");
        no("add filter.svf");
        yes("connect input.keyboard Pitch -> osc.sine V/Oct");
        yes("connect util.vca Out -> output.audio Mono");
        yes("connect osc.sine#1 out:Out -> util.vca in:In");
        no("connect osc.sine Out -> output.audio Mono");
        no("connect input.keyboard Gate -> util.vca CV");
        // First Sound's oscillator plays a triangle
        yes("param osc.sine Waveform = Tri");
        no("param osc.sine Waveform = Saw");
        yes("param osc.sine Waveform = 3");
        yes("param mod.adsr Attack < 0.01");
        no("param mod.adsr Attack > 0.01");
        no("param mod.adsr Attack changed");
        no("group");
        yes("leave");
        no("enter");
        no("next");
    }

    #[test]
    fn a_cable_counts_through_groups() {
        // Basic Subtractive's voice is in no group, but Orrery's are
        let (graph, state) = staged(include_str!("../../patches/orrery.json"));
        assert!(check("group", &graph, &state));
        // Every cable into the Audio Output comes out of a group's jack
        let into_output = groups::leaf_cables(&graph)
            .into_iter()
            .find(|(_, input)| graph.nodes[graph.get_input(*input).node].user_data.module_id == "output.audio")
            .expect("the output is patched");
        let source = &graph.nodes[graph.get_output(into_output.0).node];
        let out = source.outputs.iter().find(|(_, id)| *id == into_output.0).unwrap().0.clone();
        let input = graph.nodes[graph.get_input(into_output.1).node].inputs.iter().find(|(_, id)| *id == into_output.1).unwrap().0.clone();
        assert!(check(&format!("connect {} {out} -> output.audio {input}", source.user_data.module_id), &graph, &state));
    }

    #[test]
    fn timed_waits_count_from_where_the_lesson_reached_them() {
        let (graph, state) = staged(include_str!("../../patches/first-sound.json"));
        let facts = |playing, notes_held| Facts { graph: &graph, state: &state, playing, notes_held, probing: false };
        let mut p = Progress::default();
        let listen = Wait::Listen(1.0);
        assert!(!listen.met(&facts(false, 0), &mut p, 5.0), "stopped time isn't listening");
        assert!(!listen.met(&facts(true, 0), &mut p, 0.6));
        assert_eq!(listen.fraction(&p), Some(0.6));
        assert!(listen.met(&facts(true, 0), &mut p, 0.6));

        // A note held as the wait begins isn't one played for it
        let mut p = Progress::default();
        let notes = Wait::Notes(2);
        assert!(!notes.met(&facts(false, 1), &mut p, 0.0));
        assert!(!notes.met(&facts(false, 2), &mut p, 0.0), "one more key");
        assert!(!notes.met(&facts(false, 0), &mut p, 0.0));
        assert_eq!(notes.fraction(&p), Some(0.5));
        assert!(notes.met(&facts(false, 1), &mut p, 0.0));
    }

    #[test]
    fn a_star_stands_for_any_run_of_characters() {
        assert!(matches("Step * Pitch", "Step 12 Pitch"));
        assert!(matches("step * pitch", "Step 1 Pitch"));
        assert!(!matches("Step * Pitch", "Step 1 Gate"));
        assert!(matches("Cutoff", "cutoff") && !matches("Cutoff", "Cutoff CV"));
        assert!(matches("*", "anything"));
    }

    #[test]
    fn changed_is_from_the_value_as_the_wait_began() {
        let (mut graph, state) = staged(include_str!("../../patches/first-sound.json"));
        let wait = Wait::Param {
            module: ModuleRef { kind: "mod.adsr".into(), which: None },
            input: "Sustain".into(),
            test: Test::Changed,
        };
        let mut p = Progress::default();
        let met = |graph: &SynthGraph, p: &mut Progress| {
            wait.met(&Facts { graph, state: &state, playing: false, notes_held: 0, probing: false }, p, 0.0)
        };
        assert!(!met(&graph, &mut p));
        let node = graph.nodes.iter().find(|(_, n)| n.user_data.module_id == "mod.adsr").unwrap().0;
        let id = graph.nodes[node].inputs.iter().find(|(n, _)| n == "Sustain").unwrap().1;
        graph.inputs[id].value.set_actual_value(0.1);
        assert!(met(&graph, &mut p));
    }
}
