//! The Arranger's timeline, drawn beside its jacks.
//!
//! The song runs left to right, each section as wide as its share of the
//! bars (with a little room kept for short ones), and a playhead sweeps it.
//!
//! - The **sections** sit on the five rows of the song's own jacks (Section,
//!   Section Trig, Bar, Last Bar, End) as named blocks, the playing one lit
//!   green and its final bar shaded, where fills go.
//! - Each **lane** sits on its two rows, CV and Gate, so it reads straight
//!   across into its cables: its level as an orange line over the song, and
//!   along the bottom a green strip wherever its gate is high, with a tick
//!   for each Hit.
//!
//! Drag a lane up or down in a section to set where that section takes it,
//! or Shift-drag to set where its ramp starts; right-click for how it gets
//! there (Hold, Jump, Ramp, Hit, and a ramp's curve and start). Click a section
//! to pick it for the bar under the timeline, double-click to rename it.
//! Every edit is one undo step.
//!
//! An Arranger that follows another (see [`super::arranger_follow`]) has
//! its leader's sections, so they can be picked but not edited, and the bar
//! says whose they are. A section added, copied, moved or deleted on a
//! leader is on its followers too, with their own cues.

use eframe::egui::{self, Align2, Color32, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::{InputId, NodeId};

use crate::app::theme;
use crate::modules::arranger::{Arranger as Arr, Cue, Curve, Move, Position, FROM_WHERE_IT_IS, GATE_NAMES, LANES, LANE_NAMES, MAX_BARS, MAX_GLIDE, SECTIONS};

use super::arranger_follow::{self, NAME_KEY};
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// Height of one output row, in unzoomed points.
const ROW: f32 = 13.0;
/// Room for the lane names, left of the timeline.
const LABEL: f32 = 52.0;
/// The timeline's width.
const TIMELINE: f32 = 440.0;
/// Room for the jack labels, right of the timeline.
const JACK_LABEL: f32 = 44.0;
/// The narrowest a section is drawn, however short.
const MIN_SECTION: f32 = 10.0;
/// The song's own outputs, top to bottom, with the short labels they wear.
const SONG_OUTPUTS: [(&str, &str); 5] = [("Section", "Section"), ("Section Trig", "Trig"), ("Bar", "Bar"), ("Last Bar", "Last"), ("End", "End")];

/// The key a section's name is saved under, 0-based.
pub fn section_key(section: usize) -> String {
    format!("Section {}", section + 1)
}

/// The key a lane's name is saved under, 0-based.
pub fn lane_key(lane: usize) -> String {
    format!("Lane {}", lane + 1)
}

/// The node's parameters, by index.
struct Params<'a> {
    graph: &'a SynthGraph,
    inputs: &'a [(String, InputId)],
}

impl<'a> Params<'a> {
    fn of(graph: &'a SynthGraph, node_id: NodeId) -> Option<Self> {
        let node = graph.nodes.get(node_id)?;
        let first = node.inputs.iter().position(|(name, _)| name == "Steps")?;
        let inputs = &node.inputs[first..];
        (inputs.len() >= Arr::PARAM_COUNT).then_some(Self { graph, inputs })
    }

    fn values(&self) -> Vec<f32> {
        self.inputs[..Arr::PARAM_COUNT].iter().map(|(_, id)| self.graph.get_input(*id).value.actual_value()).collect()
    }

    fn name(&self, index: usize) -> String {
        self.inputs[index].0.clone()
    }
}

fn edit(node_id: NodeId, label: impl Into<String>, changes: Vec<(String, f32)>) -> SynthResponse {
    SynthResponse::EditParameters { node_id, label: label.into(), changes }
}

/// The parameter changes that store a cue: its own and its From.
fn cue_changes(params: &Params, section: usize, lane: usize, cue: Cue) -> Vec<(String, f32)> {
    vec![(params.name(Arr::cue_param(section, lane)), cue.encode()), (params.name(Arr::from_param(section, lane)), cue.encode_from())]
}

/// The song as the parameters have it: where each section starts, in bars.
struct Song {
    values: Vec<f32>,
    steps: usize,
    /// Each section's first bar and length in bars.
    sections: Vec<(usize, usize)>,
    bars: usize,
}

impl Song {
    fn of(values: Vec<f32>) -> Self {
        let mut sections = Vec::new();
        let mut bars = 0;
        for section in 0..Arr::sections(&values) {
            let length = Arr::length(&values, section);
            sections.push((bars, length));
            bars += length;
        }
        Self { steps: Arr::steps(&values), values, sections, bars }
    }

    fn count(&self) -> usize {
        self.sections.len()
    }

    fn cue(&self, section: usize, lane: usize) -> Cue {
        Arr::cue(&self.values, section, lane)
    }

    /// Where a lane starts each section, before the section's cue: its
    /// level, and the ramp it's on (from, to, first bar, bars).
    fn lane_starts(&self, lane: usize) -> Vec<LaneState> {
        let mut state = LaneState::default();
        let mut starts = Vec::with_capacity(self.count());
        for (section, &(start, length)) in self.sections.iter().enumerate() {
            starts.push(state);
            state = state.cued(self.cue(section, lane), start as f32, length as f32);
        }
        starts
    }
}

/// A ramp as the timeline draws it, in bars rather than clocks.
#[derive(Clone, Copy, Debug)]
struct DrawnRamp {
    from: f32,
    to: f32,
    curve: Curve,
    start: f32,
    bars: f32,
}

/// A lane's level as the timeline works it out.
#[derive(Clone, Copy, Debug, Default)]
struct LaneState {
    level: f32,
    ramp: Option<DrawnRamp>,
}

impl LaneState {
    fn at(&self, bar: f32) -> f32 {
        match self.ramp {
            Some(r) if bar < r.start + r.bars => r.from + (r.to - r.from) * r.curve.shape((bar - r.start) / r.bars),
            Some(r) => r.to,
            None => self.level,
        }
    }

    /// The state once a section starting at `start` has given its cue.
    fn cued(self, cue: Cue, start: f32, length: f32) -> Self {
        match cue.kind {
            Move::Hold | Move::Hit => self,
            Move::Jump => LaneState { level: cue.value(), ramp: None },
            Move::Ramp => {
                let from = cue.from_value().unwrap_or_else(|| self.at(start));
                let bars = if cue.bars == 0 { length } else { cue.bars as f32 };
                LaneState { level: from, ramp: Some(DrawnRamp { from, to: cue.value(), curve: cue.curve, start, bars: bars.max(1.0) }) }
            }
        }
    }
}

/// Where the sections fall across the timeline.
struct Layout {
    /// Each section's left edge, then the last one's right edge.
    edges: Vec<f32>,
}

impl Layout {
    fn of(song: &Song, left: f32, width: f32, z: f32) -> Self {
        let n = song.count().max(1);
        let floor = (MIN_SECTION * z).min(width / n as f32);
        let spare = width - floor * n as f32;
        let mut edges = Vec::with_capacity(n + 1);
        let mut x = left;
        edges.push(x);
        for &(_, length) in &song.sections {
            x += floor + spare * length as f32 / song.bars.max(1) as f32;
            edges.push(x);
        }
        Self { edges }
    }

    /// The x of a point `bar` bars (fractional) into a section.
    fn x(&self, song: &Song, section: usize, bar: f32) -> f32 {
        let (left, right) = (self.edges[section], self.edges[section + 1]);
        left + (right - left) * (bar / song.sections[section].1 as f32).clamp(0.0, 1.0)
    }

    fn span(&self, section: usize) -> (f32, f32) {
        (self.edges[section], self.edges[section + 1])
    }
}

/// The section picked for editing. It's the editor's choice, not the
/// patch's, so it isn't saved and isn't an edit.
fn selected(ctx: &egui::Context, node_id: NodeId) -> usize {
    ctx.data(|data| data.get_temp::<usize>(Id::new((node_id, "arranger-selected")))).unwrap_or(0)
}

fn select(ctx: &egui::Context, node_id: NodeId, section: usize) {
    ctx.data_mut(|data| data.insert_temp(Id::new((node_id, "arranger-selected")), section));
}

/// A name being typed: what it names, and the text so far.
#[derive(Clone, Debug, PartialEq)]
struct Renaming {
    key: String,
    text: String,
    /// The field has been shown, focused and its text selected.
    opened: bool,
}

impl Renaming {
    fn start(key: String, text: String) -> Self {
        Self { key, text, opened: false }
    }
}

fn renaming_id(node_id: NodeId) -> Id {
    Id::new((node_id, "arranger-renaming"))
}

fn position(user_state: &SynthGraphState, node_id: NodeId) -> Option<Position> {
    let engine_id = user_state.get_engine_node_id(node_id)?;
    user_state.readouts.get(&engine_id).map(Position::from_readout)
}

fn output_value(user_state: &SynthGraphState, node_id: NodeId, output: usize) -> Option<f32> {
    user_state.get_engine_node_id(node_id).and_then(|engine_id| user_state.get_output_value(engine_id, output))
}

/// A section's name: the one it was given, or its number.
fn section_name(graph: &SynthGraph, node_id: NodeId, section: usize) -> String {
    graph.nodes.get(node_id).and_then(|node| node.user_data.labels.get(&section_key(section)).cloned()).unwrap_or_else(|| format!("{}", section + 1))
}

/// A lane's name: the one it was given, or what its cable drives, or None.
fn lane_name(graph: &SynthGraph, node_id: NodeId, lane: usize) -> (Option<String>, bool) {
    let Some(node) = graph.nodes.get(node_id) else { return (None, false) };
    if let Some(name) = node.user_data.labels.get(&lane_key(lane)) {
        return (Some(name.clone()), true);
    }
    for port in [LANE_NAMES[lane], GATE_NAMES[lane]] {
        let Ok(output) = node.get_output(port) else { continue };
        if let Some((input, _)) = graph.iter_connections().find(|(_, from)| *from == output) {
            let input = graph.get_input(input);
            if let Some(target) = graph.nodes.get(input.node) {
                let port = target.inputs.iter().find(|(_, id)| *id == input.id).map_or("", |(name, _)| name.as_str());
                return (Some(format!("{} {port}", target.label)), false);
            }
        }
    }
    (None, false)
}

/// The text of the Arranger's header note: its name, if it was given one,
/// and the section playing.
pub fn title_note(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId) -> Option<String> {
    let position = position(user_state, node_id).filter(|p| p.started);
    let playing = position.map(|p| if p.ended { "End".to_string() } else { section_name(graph, node_id, p.section) });
    match (arranger_follow::given_name(graph, node_id), playing) {
        (Some(name), Some(playing)) => Some(format!("{name}  ▸  {playing}")),
        (Some(name), None) => Some(name),
        (None, Some(playing)) => Some(format!("▸  {playing}")),
        (None, None) => None,
    }
}

/// Shortens text to fit a width, with an ellipsis.
fn fit(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> String {
    let wide = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x;
    if wide(text) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let shorter: String = chars.iter().collect::<String>() + "…";
        if wide(&shorter) <= width {
            return shorter;
        }
    }
    String::new()
}

/// A level in words.
fn percent(level: f32) -> String {
    format!("{}%", (level * 100.0).round())
}

/// How a cue reads in words.
fn describe(cue: Cue) -> String {
    let level = percent(cue.value());
    match cue.kind {
        Move::Hold => "holds".to_string(),
        Move::Jump => format!("jumps to {level}"),
        Move::Ramp => {
            let from = cue.from_value().map_or(String::new(), |from| format!(" from {}", percent(from)));
            let over = match cue.bars {
                0 => "over the section".to_string(),
                1 => "over a bar".to_string(),
                bars => format!("over {bars} bars"),
            };
            let curve = match cue.curve {
                Curve::S => String::new(),
                Curve::Linear => ", in a line".to_string(),
                Curve::Exp => ", slow then fast".to_string(),
                Curve::Log => ", fast then slow".to_string(),
            };
            format!("ramps{from} to {level} {over}{curve}")
        }
        Move::Hit => format!("hits at {level}"),
    }
}

/// Which output row is being drawn.
#[derive(Clone, Copy)]
enum Row {
    /// The first of the song's rows, which draws the sections over all five.
    Sections,
    /// Another of the song's rows.
    Song,
    /// A lane's CV row, which draws the lane over its Gate row too.
    Lane(usize),
    /// A lane's Gate row.
    Gate,
}

/// Draws the output row for `output`, with its part of the timeline, and
/// returns the edits made on it. `None` if this isn't an Arranger's output,
/// so the row is drawn as usual.
pub fn output_row(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    output: &str,
) -> Option<Vec<SynthResponse>> {
    let (row, short) = if output == SONG_OUTPUTS[0].0 {
        (Row::Sections, SONG_OUTPUTS[0].1)
    } else if let Some((_, short)) = SONG_OUTPUTS.iter().find(|(name, _)| *name == output) {
        (Row::Song, *short)
    } else if let Some(lane) = LANE_NAMES.iter().position(|name| *name == output) {
        (Row::Lane(lane), "CV")
    } else {
        GATE_NAMES.iter().position(|name| *name == output).map(|_| (Row::Gate, "Gate"))?
    };
    let params = Params::of(graph, node_id)?;
    let z = user_state.zoom;

    let (rect, _) = ui.allocate_exact_size(Vec2::new((LABEL + TIMELINE + JACK_LABEL) * z, ROW * z), Sense::hover());

    // The jack's label, small enough for the rows, flush with the edge
    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let galley = ui.painter().layout_no_wrap(short.to_string(), FontId::proportional(10.0 * z), ink);
    let label_rect = Rect::from_min_size(Pos2::new(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0), galley.size());
    super::node_data::defer_output_label(ui, node_id, output, label_rect, vec![Shape::galley(label_rect.min, galley, ink)]);

    // The song's triggers light a lamp beside their jacks
    let lamp = match output {
        "Section Trig" => Some((Arr::OUT_SECTION_TRIG, theme::signal::GATE)),
        "Bar" => Some((Arr::OUT_BAR, theme::signal::GATE)),
        "Last Bar" => Some((Arr::OUT_LAST_BAR, theme::signal::GATE)),
        "End" => Some((Arr::OUT_END, theme::signal::GATE)),
        _ => None,
    };
    if let Some((index, color)) = lamp {
        let c = Pos2::new(rect.right() - JACK_LABEL * z + 6.0 * z, rect.center().y);
        let on = output_value(user_state, node_id, index).is_some_and(|v| v > 0.5);
        ui.painter().circle_filled(c, 2.4 * z, if on { color } else { theme::background::WIDGET_ACTIVE });
        if on {
            ui.painter().circle_filled(c, 5.0 * z, color.gamma_multiply(0.25));
        }
    }

    let pitch = rect.height() + ui.spacing().item_spacing.y;
    let rows_below = match row {
        Row::Sections => SONG_OUTPUTS.len() - 1,
        Row::Lane(_) => 1,
        _ => return Some(Vec::new()),
    };
    let block = Rect::from_min_max(rect.left_top(), Pos2::new(rect.right() - JACK_LABEL * z, rect.bottom() + rows_below as f32 * pitch));
    if !ui.is_rect_visible(block) {
        return Some(Vec::new());
    }

    let song = Song::of(params.values());
    let timeline = Rect::from_min_max(Pos2::new(block.left() + LABEL * z, block.top()), block.max);
    let layout = Layout::of(&song, timeline.left(), timeline.width(), z);
    let position = position(user_state, node_id).filter(|p| p.started && p.section < song.count());
    let selected = selected(ui.ctx(), node_id).min(song.count() - 1);
    let mut edits = Vec::new();
    match row {
        Row::Sections => sections(ui, node_id, graph, user_state, &params, &song, &layout, block, timeline, position, selected, &mut edits),
        Row::Lane(lane) => lane_rows(ui, node_id, graph, user_state, &params, &song, &layout, block, timeline, lane, position, selected, &mut edits),
        _ => {}
    }

    // The playhead, through every row
    if let Some(p) = position {
        let bar = p.bar as f32 + p.step / song.steps as f32;
        let x = layout.x(&song, p.section, bar);
        let gap = ui.spacing().item_spacing.y;
        let span = egui::Rangef::new(timeline.top() - gap / 2.0, timeline.bottom() + gap / 2.0);
        let strength = if p.ended { 0.25 } else { 0.8 };
        ui.painter().vline(x, span, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(strength)));
    }
    Some(edits)
}

/// The song's rows: a bar ruler, then a block for each section.
#[allow(clippy::too_many_arguments)]
fn sections(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    params: &Params,
    song: &Song,
    layout: &Layout,
    block: Rect,
    timeline: Rect,
    position: Option<Position>,
    selected: usize,
    edits: &mut Vec<SynthResponse>,
) {
    let z = user_state.zoom;
    let painter = ui.painter().clone();
    let green = theme::signal::GATE;

    // Where the song is, at the left: bar of bars
    let font = FontId::proportional(9.0 * z);
    let big = FontId::proportional(15.0 * z);
    let left = block.left() + 2.0 * z;
    let bar_now = position.map(|p| song.sections[p.section].0 + p.bar + 1);
    let (number, number_ink) = match bar_now {
        Some(bar) => (bar.to_string(), theme::text::PRIMARY),
        None => ("–".to_string(), theme::text::DISABLED),
    };
    painter.text(Pos2::new(left, block.top() + 2.0 * z), Align2::LEFT_TOP, "BAR", font.clone(), theme::text::DISABLED);
    painter.text(Pos2::new(left, block.top() + 12.0 * z), Align2::LEFT_TOP, number, big, number_ink);
    painter.text(Pos2::new(left, block.top() + 30.0 * z), Align2::LEFT_TOP, format!("of {}", song.bars), font.clone(), theme::text::SECONDARY);
    // The section playing, under the bar count
    if let Some(p) = position {
        let name = if p.ended { "End".to_string() } else { section_name(graph, node_id, p.section) };
        let text = fit(&painter, &name, &font, LABEL * z - 6.0 * z);
        painter.text(Pos2::new(left, block.top() + 44.0 * z), Align2::LEFT_TOP, text, font.clone(), green);
    }

    // The ruler: a tick a bar if they'd be far enough apart, and a number
    // where each section starts
    let ruler = Rect::from_min_max(timeline.left_top(), Pos2::new(timeline.right(), timeline.top() + 11.0 * z));
    for (section, &(start, length)) in song.sections.iter().enumerate() {
        let (x0, x1) = layout.span(section);
        let per_bar = (x1 - x0) / length as f32;
        if per_bar >= 3.0 * z {
            for bar in 1..length {
                let x = x0 + per_bar * bar as f32;
                painter.vline(x, egui::Rangef::new(ruler.bottom() - 2.5 * z, ruler.bottom()), Stroke::new(1.0, theme::background::GRID_MAJOR));
            }
        }
        painter.vline(x0, egui::Rangef::new(ruler.top() + 2.0 * z, ruler.bottom()), Stroke::new(1.0, theme::text::DISABLED));
        if x1 - x0 >= 14.0 * z {
            painter.text(Pos2::new(x0 + 2.0 * z, ruler.top()), Align2::LEFT_TOP, (start + 1).to_string(), FontId::monospace(7.5 * z), theme::text::DISABLED);
        }
    }

    // The blocks
    let blocks = Rect::from_min_max(Pos2::new(timeline.left(), ruler.bottom() + 2.0 * z), Pos2::new(timeline.right(), timeline.bottom() - 1.0 * z));
    let renaming: Option<Renaming> = ui.ctx().data(|data| data.get_temp(renaming_id(node_id)));
    for section in 0..song.count() {
        let (x0, x1) = layout.span(section);
        let rect = Rect::from_min_max(Pos2::new(x0 + 0.75 * z, blocks.top()), Pos2::new(x1 - 0.75 * z, blocks.bottom()));
        let playing = position.is_some_and(|p| p.section == section && !p.ended);
        let queued = position.is_some_and(|p| p.queued == Some(section));
        let round = 3.0 * z;
        let fill = if playing {
            green.gamma_multiply(0.30)
        } else if section % 2 == 0 {
            theme::background::WIDGET
        } else {
            theme::background::WIDGET.lerp_to_gamma(theme::background::PANEL, 0.4)
        };
        painter.rect_filled(rect, round, fill);
        // The last bar, where a fill goes
        let length = song.sections[section].1;
        if length > 1 {
            let last = Rect::from_min_max(Pos2::new(layout.x(song, section, (length - 1) as f32), rect.top()), rect.max);
            painter.rect_filled(last, round, Color32::BLACK.gamma_multiply(0.18));
        }
        if playing {
            painter.hline(rect.x_range(), rect.top() + 0.75 * z, Stroke::new(1.5 * z, green));
        }
        if queued {
            painter.rect_stroke(rect, round, Stroke::new(1.0 * z, green.gamma_multiply(0.8)));
        }
        if section == selected {
            painter.rect_stroke(rect.expand(0.5 * z), round, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(0.75)));
        }

        let name = section_name(graph, node_id, section);
        let key = section_key(section);
        let is_renaming = renaming.as_ref().is_some_and(|r| r.key == key);
        if !is_renaming {
            let name_font = FontId::proportional(10.0 * z);
            let text = fit(&painter, &name, &name_font, rect.width() - 4.0 * z);
            let ink = if playing { Color32::WHITE } else { theme::text::PRIMARY };
            painter.text(Pos2::new(rect.left() + 3.0 * z, rect.top() + 3.0 * z), Align2::LEFT_TOP, text, name_font, ink);
            let bars = format!("{length}");
            if rect.width() > 22.0 * z {
                painter.text(Pos2::new(rect.right() - 3.0 * z, rect.bottom() - 2.0 * z), Align2::RIGHT_BOTTOM, bars, FontId::monospace(8.0 * z), theme::text::SECONDARY);
            }
        }

        let response = ui
            .interact(rect, Id::new((node_id, "arranger-section", section)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            select(ui.ctx(), node_id, section);
        }
        let leader = arranger_follow::leader(user_state, node_id);
        if response.double_clicked() && leader.is_none() {
            let existing = graph.nodes.get(node_id).and_then(|n| n.user_data.labels.get(&key).cloned()).unwrap_or_default();
            ui.ctx().data_mut(|data| data.insert_temp(renaming_id(node_id), Renaming::start(key.clone(), existing)));
        }
        let how = match leader {
            Some(leader) => format!("Click to pick · it's {}'s section: edit it there", arranger_follow::name(graph, leader)),
            None => "Click to pick · double-click to rename · right-click for more".to_string(),
        };
        let response = response.on_hover_text(format!(
            "{name}: {length} bar{}, from bar {}\n{how}",
            if length == 1 { "" } else { "s" },
            song.sections[section].0 + 1
        ));
        response.context_menu(|ui| {
            select(ui.ctx(), node_id, section);
            section_menu(ui, node_id, graph, user_state, params, song, section, edits);
        });

        if is_renaming {
            if let Some(done) = rename_field(ui, node_id, rect.shrink2(Vec2::new(1.0 * z, 2.0 * z)).with_max_y(rect.top() + 16.0 * z), z) {
                edits.extend(done);
            }
        }
    }
}

/// The name field of a section or lane being renamed, over `rect`. Returns
/// the edit once it's typed.
fn rename_field(ui: &mut egui::Ui, node_id: NodeId, rect: Rect, z: f32) -> Option<Vec<SynthResponse>> {
    let id = renaming_id(node_id);
    let mut renaming: Renaming = ui.ctx().data(|data| data.get_temp(id))?;
    let field_id = Id::new((node_id, "arranger-rename-field"));
    let mut output = ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
            egui::TextEdit::singleline(&mut renaming.text)
                .id(field_id)
                .font(FontId::proportional(10.0 * z))
                .margin(Vec2::new(2.0 * z, 0.0))
                .desired_width(rect.width())
                .show(ui)
        })
        .inner;
    let response = output.response.clone();
    // The first frame: focus it, with the old name selected so typing replaces it
    if !renaming.opened {
        renaming.opened = true;
        response.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(renaming.text.chars().count()));
        output.state.cursor.set_char_range(Some(all));
        output.state.store(ui.ctx(), field_id);
        ui.ctx().data_mut(|data| data.insert_temp(id, renaming));
        return None;
    }
    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if escape {
        ui.ctx().data_mut(|data| data.remove::<Renaming>(id));
        return None;
    }
    if response.lost_focus() || enter {
        ui.ctx().data_mut(|data| data.remove::<Renaming>(id));
        let text = renaming.text.trim().to_string();
        let what = if renaming.key == NAME_KEY { "the Arranger" } else { renaming.key.as_str() };
        let label = if text.is_empty() { format!("Clear the name of {what}") } else { format!("Name {what} {text}") };
        return Some(vec![SynthResponse::SetLabels { node_id, label, labels: vec![(renaming.key, text)] }]);
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, renaming));
    None
}

/// A lane, over its CV and Gate rows: its name, then its level through the
/// song, with a cell per section to drag.
#[allow(clippy::too_many_arguments)]
fn lane_rows(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    params: &Params,
    song: &Song,
    layout: &Layout,
    block: Rect,
    timeline: Rect,
    lane: usize,
    position: Option<Position>,
    selected: usize,
    edits: &mut Vec<SynthResponse>,
) {
    let z = user_state.zoom;
    let painter = ui.painter().clone();
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;

    // The lane's name, given or taken from its cable
    let (name, given) = lane_name(graph, node_id, lane);
    let lane_label = name.clone().unwrap_or_else(|| format!("Lane {}", lane + 1));
    let name_rect = Rect::from_min_max(block.left_top(), Pos2::new(timeline.left() - 3.0 * z, block.bottom()));
    let renaming: Option<Renaming> = ui.ctx().data(|data| data.get_temp(renaming_id(node_id)));
    let key = lane_key(lane);
    if renaming.as_ref().is_some_and(|r| r.key == key) {
        let field = Rect::from_center_size(name_rect.center(), Vec2::new(name_rect.width(), 15.0 * z));
        if let Some(done) = rename_field(ui, node_id, field, z) {
            edits.extend(done);
        }
    } else {
        let font = FontId::proportional(9.5 * z);
        let number = FontId::monospace(8.0 * z);
        painter.text(Pos2::new(name_rect.left() + 1.0 * z, name_rect.top() + 2.0 * z), Align2::LEFT_TOP, (lane + 1).to_string(), number, theme::text::DISABLED);
        let text = fit(&painter, name.as_deref().unwrap_or("–"), &font, name_rect.width() - 2.0 * z);
        let ink = if given { theme::text::PRIMARY } else if name.is_some() { theme::text::SECONDARY } else { theme::text::DISABLED };
        painter.text(Pos2::new(name_rect.left() + 1.0 * z, name_rect.center().y + 3.0 * z), Align2::LEFT_CENTER, text, font, ink);
    }
    let glide = params.values()[Arr::PARAM_GLIDE + lane];
    let name_response = ui
        .interact(name_rect, Id::new((node_id, "arranger-lane", lane)), Sense::click())
        .on_hover_text(format!("{lane_label}{}\nDouble-click to rename · right-click for its glide", if glide > 0.0 { format!(", gliding {}", super::drum_display::duration(glide)) } else { String::new() }));
    if name_response.double_clicked() {
        let existing = graph.nodes.get(node_id).and_then(|n| n.user_data.labels.get(&key).cloned()).unwrap_or_default();
        ui.ctx().data_mut(|data| data.insert_temp(renaming_id(node_id), Renaming::start(key.clone(), existing)));
    }
    name_response.context_menu(|ui| lane_menu(ui, node_id, params, song, lane, &lane_label, glide, edits));

    // The level's room, and the gate strip under it
    let strip = 3.0 * z;
    let area = Rect::from_min_max(Pos2::new(timeline.left(), block.top() + 1.0 * z), Pos2::new(timeline.right(), block.bottom() - strip - 2.0 * z));
    let strip_rect = Rect::from_min_max(Pos2::new(timeline.left(), block.bottom() - strip), Pos2::new(timeline.right(), block.bottom()));
    painter.rect_filled(area, 2.0 * z, theme::background::PANEL.gamma_multiply(0.6));
    painter.rect_filled(strip_rect, 1.5 * z, theme::background::WIDGET.gamma_multiply(0.6));
    let y_of = |level: f32| area.bottom() - level.clamp(0.0, 1.0) * area.height();

    let starts = song.lane_starts(lane);
    let mut points = Vec::new();
    let step = 2.0 * z;
    for section in 0..song.count() {
        let (x0, x1) = layout.span(section);
        let (start, length) = song.sections[section];
        let state = starts[section].cued(song.cue(section, lane), start as f32, length as f32);
        // The selected section's column, faintly lit
        if section == selected {
            painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, area.y_range()), 0.0, Color32::WHITE.gamma_multiply(0.035));
        }
        let samples = (((x1 - x0) / step).ceil() as usize).max(1);
        let mut gate_from: Option<f32> = None;
        for n in 0..=samples {
            let x = x0 + (x1 - x0) * n as f32 / samples as f32;
            let bar = start as f32 + length as f32 * n as f32 / samples as f32;
            let level = state.at(bar);
            points.push(Pos2::new(x, y_of(level)));
            match (level > 0.0005, gate_from) {
                (true, None) => gate_from = Some(x),
                (false, Some(from)) => {
                    painter.rect_filled(Rect::from_x_y_ranges(from..=x, strip_rect.y_range()), 1.0 * z, green.gamma_multiply(0.55));
                    gate_from = None;
                }
                _ => {}
            }
        }
        if let Some(from) = gate_from {
            painter.rect_filled(Rect::from_x_y_ranges(from..=x1, strip_rect.y_range()), 1.0 * z, green.gamma_multiply(0.55));
        }
        // A section boundary, faint
        if section > 0 {
            painter.vline(x0, area.y_range(), Stroke::new(1.0, theme::background::GRID_MAJOR.gamma_multiply(0.7)));
        }
    }
    // A lane no section moves is drawn faintly
    let idle = (0..song.count()).all(|section| song.cue(section, lane).kind == Move::Hold);
    let line = if idle { orange.gamma_multiply(0.3) } else { orange };
    if !idle {
        super::drum_display::fill_under(&painter, &points, area.bottom(), orange.gamma_multiply(0.22), orange.gamma_multiply(0.02));
    }
    painter.add(Shape::line(points, Stroke::new(1.4 * z, line)));

    // Hits, as ticks from the strip up to their level
    for section in 0..song.count() {
        let cue = song.cue(section, lane);
        if cue.kind == Move::Hit {
            let x = layout.span(section).0 + 1.5 * z;
            painter.vline(x, egui::Rangef::new(y_of(cue.value()), strip_rect.bottom()), Stroke::new(1.4 * z, green));
            painter.circle_filled(Pos2::new(x, y_of(cue.value())), 2.2 * z, green);
        }
    }

    // Where the lane is now, riding the playhead
    if let Some(p) = position {
        let x = layout.x(song, p.section, p.bar as f32 + p.step / song.steps as f32);
        if let Some(level) = output_value(user_state, node_id, Arr::OUT_LANES + 2 * lane) {
            let c = Pos2::new(x, y_of(level));
            painter.circle_filled(c, 5.0 * z, orange.gamma_multiply(0.25 + 0.3 * level.clamp(0.0, 1.0)));
            painter.circle_filled(c, 2.4 * z, orange.lerp_to_gamma(Color32::WHITE, 0.4));
        }
        if output_value(user_state, node_id, Arr::OUT_LANES + 2 * lane + 1).is_some_and(|v| v > 0.5) {
            painter.circle_filled(Pos2::new(x, strip_rect.center().y), 3.0 * z, green.lerp_to_gamma(Color32::WHITE, 0.4));
        }
    }

    // A cell per section: drag to set its level
    for section in 0..song.count() {
        let (x0, x1) = layout.span(section);
        let cell = Rect::from_min_max(Pos2::new(x0, block.top()), Pos2::new(x1, block.bottom()));
        let cue = song.cue(section, lane);
        let what = format!("{} · {lane_label}", section_name(graph, node_id, section));
        let response = ui
            .interact(cell, Id::new((node_id, "arranger-cell", lane, section)), Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeVertical);
        if response.clicked() {
            select(ui.ctx(), node_id, section);
        }
        if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let level = (((area.bottom() - pointer.y) / area.height()).clamp(0.0, 1.0) * 1000.0).round() as u16;
                let next = if ui.input(|input| input.modifiers.shift) {
                    // Where the ramp starts: anything else becomes a ramp to
                    // where it was going, or for a Hold, where the lane was
                    let to = if cue.kind == Move::Hold { (starts[section].at(song.sections[section].0 as f32) * 1000.0).round() as u16 } else { cue.level };
                    Cue { kind: Move::Ramp, level: to, from: Some(level), ..cue }
                } else {
                    let kind = if cue.kind == Move::Hold { Move::Jump } else { cue.kind };
                    Cue { kind, level, ..cue }
                };
                if next != cue {
                    select(ui.ctx(), node_id, section);
                    let label = format!("{what} {}", describe(next));
                    edits.push(edit(node_id, label, cue_changes(params, section, lane, next)));
                }
            }
        }
        let response = response.on_hover_text(format!(
            "{what} {}\nDrag up or down to set its level · Shift-drag to set where its ramp starts · right-click for how it gets there",
            describe(cue)
        ));
        response.context_menu(|ui| {
            select(ui.ctx(), node_id, section);
            cue_menu(ui, node_id, params, section, lane, cue, &what, edits);
        });
    }
}

/// A cue's right-click menu: how the lane moves, and to what.
#[allow(clippy::too_many_arguments)]
fn cue_menu(ui: &mut egui::Ui, node_id: NodeId, params: &Params, section: usize, lane: usize, cue: Cue, what: &str, edits: &mut Vec<SynthResponse>) {
    ui.label(egui::RichText::new(what).strong());
    ui.separator();
    let set = |next: Cue, edits: &mut Vec<SynthResponse>| {
        if next != cue {
            edits.push(edit(node_id, format!("{what} {}", describe(next)), cue_changes(params, section, lane, next)));
        }
    };
    ui.horizontal(|ui| {
        for kind in Move::ALL {
            let hint = match kind {
                Move::Hold => "Carry on as it was",
                Move::Jump => "Go straight to the level on the downbeat",
                Move::Ramp => "Glide to the level, on a curve of your choosing",
                Move::Hit => "Go to the level for half a step, then back: a trigger for a drum or a button",
            };
            if ui.selectable_label(cue.kind == kind, kind.name()).on_hover_text(hint).clicked() {
                set(Cue { kind, ..cue }, edits);
            }
        }
    });
    if cue.kind != Move::Hold {
        ui.horizontal(|ui| {
            ui.label("Level");
            let mut percent = cue.value() * 100.0;
            if ui.add(egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(0.5).suffix("%").max_decimals(1)).changed() {
                set(Cue { level: (percent * 10.0).round() as u16, ..cue }, edits);
            }
        });
    }
    if cue.kind == Move::Ramp {
        ui.horizontal(|ui| {
            ui.label("Over");
            if ui.selectable_label(cue.bars == 0, "the section").clicked() {
                set(Cue { bars: 0, ..cue }, edits);
            }
            for bars in [1u8, 2, 4, 8, 16] {
                if ui.selectable_label(cue.bars == bars, bars.to_string()).clicked() {
                    set(Cue { bars, ..cue }, edits);
                }
            }
            let mut bars = cue.bars.max(1) as u32;
            if ui.add(egui::DragValue::new(&mut bars).range(1..=MAX_BARS as u32).suffix(" bars")).changed() {
                set(Cue { bars: bars as u8, ..cue }, edits);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Curve");
            for curve in Curve::ALL {
                let hint = match curve {
                    Curve::S => "Leaves and arrives at rest, like a hand on a fader",
                    Curve::Linear => "The same pace all the way",
                    Curve::Exp => "Slow to start, fast to finish: a fifth of the way at half time, for a sweep or a build",
                    Curve::Log => "Fast to start, slow to finish: four fifths of the way at half time, for a swell that settles or a fade that tails away",
                };
                let icon = curve_icon(ui, curve, cue.curve == curve);
                let chip = ui.selectable_label(cue.curve == curve, curve.name());
                if (icon | chip).on_hover_text(hint).clicked() {
                    set(Cue { curve, ..cue }, edits);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("From");
            let hint = "Start from the level the lane has reached";
            if ui.selectable_label(cue.from.is_none(), "where it is").on_hover_text(hint).clicked() {
                set(Cue { from: None, ..cue }, edits);
            }
            let hint = "Jump to a level of its own on the downbeat, then ramp from there. Shift-drag the cell to set it";
            if ui.selectable_label(cue.from.is_some(), "a level").on_hover_text(hint).clicked() && cue.from.is_none() {
                set(Cue { from: Some(cue.level), ..cue }, edits);
            }
            if let Some(from) = cue.from {
                let mut percent = from as f32 / 10.0;
                if ui.add(egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(0.5).suffix("%").max_decimals(1)).changed() {
                    set(Cue { from: Some((percent * 10.0).round() as u16), ..cue }, edits);
                }
            }
        });
    }
}

/// A little drawing of a curve, beside its chip in the menu.
fn curve_icon(ui: &mut egui::Ui, curve: Curve, chosen: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(16.0, 12.0), Sense::click());
    let rect = rect.shrink(1.5);
    let points: Vec<Pos2> = (0..=12)
        .map(|n| {
            let t = n as f32 / 12.0;
            Pos2::new(rect.left() + rect.width() * t, rect.bottom() - rect.height() * curve.shape(t))
        })
        .collect();
    let ink = if chosen { theme::signal::CONTROL } else { theme::text::SECONDARY };
    ui.painter().add(Shape::line(points, Stroke::new(1.4, ink)));
    response
}

/// A lane's right-click menu: its glide, and clearing it.
#[allow(clippy::too_many_arguments)]
fn lane_menu(ui: &mut egui::Ui, node_id: NodeId, params: &Params, song: &Song, lane: usize, lane_label: &str, glide: f32, edits: &mut Vec<SynthResponse>) {
    ui.label(egui::RichText::new(lane_label).strong());
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Glide").on_hover_text("How long the CV takes to glide the whole way from 0 to 1, so a Jump doesn't click. Leave it at 0 for a lane that picks patterns or presses buttons");
        let mut seconds = glide;
        if ui.add(egui::DragValue::new(&mut seconds).range(0.0..=MAX_GLIDE).speed(0.01).suffix(" s").max_decimals(2)).changed() {
            edits.push(edit(node_id, format!("{lane_label} glides {}", super::drum_display::duration(seconds)), vec![(params.name(Arr::PARAM_GLIDE + lane), seconds)]));
        }
    });
    if ui.button("Rename…").clicked() {
        ui.ctx().data_mut(|data| data.insert_temp(renaming_id(node_id), Renaming::start(lane_key(lane), String::new())));
        ui.close_menu();
    }
    if ui.button("Hold in every section").on_hover_text("Clears the lane: it stays at 0").clicked() {
        let changes = (0..song.count()).flat_map(|section| cue_changes(params, section, lane, Cue::HOLD)).collect();
        edits.push(edit(node_id, format!("Clear {lane_label}"), changes));
        ui.close_menu();
    }
}

/// A section's right-click menu.
#[allow(clippy::too_many_arguments)]
fn section_menu(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    params: &Params,
    song: &Song,
    section: usize,
    edits: &mut Vec<SynthResponse>,
) {
    let name = section_name(graph, node_id, section);
    ui.label(egui::RichText::new(format!("Section {}: {name}", section + 1)).strong());
    ui.separator();
    match arranger_follow::leader(user_state, node_id) {
        Some(leader) => {
            ui.label(follows_text(graph, leader));
            ui.label(egui::RichText::new(follows_hint(graph, leader)).small().color(theme::text::SECONDARY));
        }
        None => section_controls(ui, node_id, graph, user_state, params, song, section, edits, true),
    }
}

/// What a follower shows in place of its section controls.
fn follows_text(graph: &SynthGraph, leader: NodeId) -> egui::RichText {
    egui::RichText::new(format!("Follows {}", arranger_follow::name(graph, leader))).color(theme::signal::CONTROL)
}

/// What a follower's "Follows" says when it's hovered.
fn follows_hint(graph: &SynthGraph, leader: NodeId) -> String {
    let leader = arranger_follow::name(graph, leader);
    format!("Its sections, Steps and Loop are {leader}'s, through the cable into Follow: edit them on {leader}. Its lanes are its own.\nUnpatch Follow to edit its sections here; it keeps them")
}

/// The bar under the timeline: the section picked, its length, and the
/// buttons that add, copy, move and remove sections.
pub fn section_bar(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, zoom: f32) -> Vec<SynthResponse> {
    let Some(params) = Params::of(graph, node_id) else { return Vec::new() };
    let song = Song::of(params.values());
    let section = selected(ui.ctx(), node_id).min(song.count() - 1);
    let mut edits = Vec::new();
    ui.add_space(4.0 * zoom);
    ui.horizontal(|ui| {
        ui.style_mut().spacing.item_spacing.x = 4.0 * zoom;
        name_field(ui, node_id, graph, LABEL * zoom - ui.spacing().item_spacing.x, zoom, &mut edits);
        if ui.small_button("◀").on_hover_text("The section before").clicked() {
            select(ui.ctx(), node_id, section.saturating_sub(1));
        }
        if ui.small_button("▶").on_hover_text("The section after").clicked() {
            select(ui.ctx(), node_id, (section + 1).min(song.count() - 1));
        }
        let name = section_name(graph, node_id, section);
        let playing = position(user_state, node_id).is_some_and(|p| p.started && !p.ended && p.section == section);
        let ink = if playing { theme::signal::GATE } else { theme::text::PRIMARY };
        ui.label(egui::RichText::new(format!("{} · {name}", section + 1)).color(ink).strong());
        match arranger_follow::leader(user_state, node_id) {
            Some(leader) => {
                let length = song.sections[section].1;
                ui.label(egui::RichText::new(format!("{length} bar{}", if length == 1 { "" } else { "s" })).color(theme::text::SECONDARY));
                ui.add_space(6.0 * zoom);
                ui.label(follows_text(graph, leader)).on_hover_text(follows_hint(graph, leader));
            }
            None => section_controls(ui, node_id, graph, user_state, &params, &song, section, &mut edits, false),
        }
    });
    edits
}

/// The Arranger's own name, at the left of the section bar under its
/// lanes' names: click to give it one, which its followers call it by.
fn name_field(ui: &mut egui::Ui, node_id: NodeId, graph: &SynthGraph, width: f32, z: f32, edits: &mut Vec<SynthResponse>) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 16.0 * z), Sense::click());
    let renaming: Option<Renaming> = ui.ctx().data(|data| data.get_temp(renaming_id(node_id)));
    if renaming.is_some_and(|r| r.key == NAME_KEY) {
        if let Some(done) = rename_field(ui, node_id, rect, z) {
            edits.extend(done);
        }
        return;
    }
    let given = arranger_follow::given_name(graph, node_id);
    let font = FontId::proportional(10.0 * z);
    let (text, ink) = match &given {
        Some(name) => (fit(ui.painter(), name, &font, rect.width() - 2.0 * z), theme::text::PRIMARY),
        None => ("Name…".to_string(), theme::text::DISABLED),
    };
    let ink = if response.hovered() { ink.lerp_to_gamma(Color32::WHITE, 0.3) } else { ink };
    ui.painter().text(Pos2::new(rect.left() + 1.0 * z, rect.center().y), Align2::LEFT_CENTER, text, font, ink);
    let response = response.on_hover_cursor(CursorIcon::Text).on_hover_text(match &given {
        Some(name) => format!("{name}: click to rename this Arranger"),
        None => "Click to name this Arranger, so the ones that follow it can say so".to_string(),
    });
    if response.clicked() {
        ui.ctx().data_mut(|data| data.insert_temp(renaming_id(node_id), Renaming::start(NAME_KEY.to_string(), given.unwrap_or_default())));
    }
}

/// A section's length and the buttons for it, in the bar or its menu.
#[allow(clippy::too_many_arguments)]
fn section_controls(
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &SynthGraphState,
    params: &Params,
    song: &Song,
    section: usize,
    edits: &mut Vec<SynthResponse>,
    menu: bool,
) {
    let name = section_name(graph, node_id, section);
    let length = song.sections[section].1;
    let mut bars = length as u32;
    let suffix = if length == 1 { " bar" } else { " bars" };
    let drag = egui::DragValue::new(&mut bars).range(1..=MAX_BARS as u32).suffix(suffix).speed(0.1);
    if ui.add(drag).on_hover_text("How many bars the section lasts").changed() {
        edits.push(edit(node_id, format!("{name} lasts {bars} bars"), vec![(params.name(Arr::PARAM_LENGTH + section), bars as f32)]));
    }
    let count = song.count();
    let full = count >= SECTIONS;
    let mut action = |ui: &mut egui::Ui, text: &str, hint: &str, enabled: bool, edit: &dyn Fn() -> Option<Vec<SynthResponse>>| {
        let button = if menu { ui.add_enabled(enabled, egui::Button::new(text)) } else { ui.add_enabled(enabled, egui::Button::new(text).small()) };
        if button.on_hover_text(hint).clicked() {
            if let Some(more) = edit() {
                edits.extend(more);
            }
            if menu {
                ui.close_menu();
            }
        }
    };
    let ctx = ui.ctx().clone();
    let chain = |edit: &dyn Fn(NodeId, &Params, &Song) -> Vec<SynthResponse>| Some(down_the_chain(node_id, graph, user_state, edit));
    action(ui, "+", "Add a section after this one", !full, &|| {
        select(&ctx, node_id, section + 1);
        chain(&|id, params, song| insert(id, graph, params, song, section + 1, None))
    });
    action(ui, "Copy", "Add a copy of this section after it", !full, &|| {
        select(&ctx, node_id, section + 1);
        chain(&|id, params, song| insert(id, graph, params, song, section + 1, Some(section)))
    });
    action(ui, "◀ Move", "Swap with the section before", section > 0, &|| {
        select(&ctx, node_id, section - 1);
        chain(&|id, params, song| swap(id, graph, params, song, section - 1))
    });
    action(ui, "Move ▶", "Swap with the section after", section + 1 < count, &|| {
        select(&ctx, node_id, section + 1);
        chain(&|id, params, song| swap(id, graph, params, song, section))
    });
    action(ui, "Delete", "Take this section out of the song", count > 1, &|| {
        select(&ctx, node_id, section.saturating_sub(1).min(count - 2));
        chain(&|id, params, song| remove(id, graph, params, song, section))
    });
}

/// A change to the sections made on an Arranger and on every one down the
/// chain that follows it, each with its own cues, so the cues move with
/// their sections. `arranger_follow` then copies the lengths and names.
fn down_the_chain(node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, edit: &dyn Fn(NodeId, &Params, &Song) -> Vec<SynthResponse>) -> Vec<SynthResponse> {
    std::iter::once(node_id)
        .chain(arranger_follow::followers(&user_state.arranger_leaders, node_id))
        .filter_map(|id| Params::of(graph, id).map(|params| (id, params)))
        .flat_map(|(id, params)| edit(id, &params, &Song::of(params.values())))
        .collect()
}

/// What one section holds: its length, its cues and its name.
#[derive(Clone)]
struct SectionData {
    length: f32,
    cues: [f32; LANES],
    froms: [f32; LANES],
    name: String,
}

impl SectionData {
    /// A new section: four bars that hold every lane.
    fn blank() -> Self {
        Self { length: 4.0, cues: [Cue::HOLD.encode(); LANES], froms: [FROM_WHERE_IT_IS; LANES], name: String::new() }
    }
}

fn section_data(graph: &SynthGraph, node_id: NodeId, song: &Song, section: usize) -> SectionData {
    let labels = graph.nodes.get(node_id).map(|n| n.user_data.labels.clone()).unwrap_or_default();
    if section >= song.count() {
        return SectionData::blank();
    }
    SectionData {
        length: song.values[Arr::PARAM_LENGTH + section],
        cues: std::array::from_fn(|lane| song.values[Arr::cue_param(section, lane)]),
        froms: std::array::from_fn(|lane| song.values[Arr::from_param(section, lane)]),
        name: labels.get(&section_key(section)).cloned().unwrap_or_default(),
    }
}

/// The edits that lay `sections` out from section `from` on, with the new
/// section count and Loop.
fn write_sections(node_id: NodeId, params: &Params, from: usize, sections: &[SectionData], count: usize, loop_to: f32, label: String) -> Vec<SynthResponse> {
    let mut changes = vec![(params.name(Arr::PARAM_SECTIONS), count as f32), (params.name(Arr::PARAM_LOOP), loop_to)];
    let mut labels = Vec::new();
    for (offset, data) in sections.iter().enumerate() {
        let section = from + offset;
        if section >= SECTIONS {
            break;
        }
        changes.push((params.name(Arr::PARAM_LENGTH + section), data.length));
        for lane in 0..LANES {
            changes.push((params.name(Arr::cue_param(section, lane)), data.cues[lane]));
            changes.push((params.name(Arr::from_param(section, lane)), data.froms[lane]));
        }
        labels.push((section_key(section), data.name.clone()));
    }
    vec![edit(node_id, label.clone(), changes), SynthResponse::SetLabels { node_id, label, labels }]
}

/// Adds a section at `at`: a copy of `copy`, or a blank one.
fn insert(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize, copy: Option<usize>) -> Vec<SynthResponse> {
    let count = song.count();
    let mut new = match copy {
        Some(section) => section_data(graph, node_id, song, section),
        None => SectionData::blank(),
    };
    let label = match copy {
        Some(section) => format!("Copy section {}", section_name(graph, node_id, section)),
        None => format!("Add section {}", at + 1),
    };
    if copy.is_some() && !new.name.is_empty() {
        new.name = format!("{} 2", new.name);
    }
    let mut moved = vec![new];
    moved.extend((at..count).map(|section| section_data(graph, node_id, song, section)));
    // Loop follows the section it named
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize - 1 >= at { loop_to + 1.0 } else { loop_to };
    write_sections(node_id, params, at, &moved, count + 1, loop_to, label)
}

/// Takes section `at` out.
fn remove(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize) -> Vec<SynthResponse> {
    let count = song.count();
    let label = format!("Delete section {}", section_name(graph, node_id, at));
    let mut moved: Vec<SectionData> = (at + 1..count).map(|section| section_data(graph, node_id, song, section)).collect();
    // The emptied slot at the end goes back to a blank section
    moved.push(SectionData::blank());
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize - 1 > at { loop_to - 1.0 } else { loop_to.min((count - 1) as f32) };
    write_sections(node_id, params, at, &moved, count - 1, loop_to, label)
}

/// Swaps section `at` with the one after it.
fn swap(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize) -> Vec<SynthResponse> {
    let label = format!("Move section {}", section_name(graph, node_id, at));
    let moved = vec![section_data(graph, node_id, song, at + 1), section_data(graph, node_id, song, at)];
    write_sections(node_id, params, at, &moved, song.count(), song.values[Arr::PARAM_LOOP], label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::DspModule;

    fn song(lengths: &[usize], cues: &[(usize, usize, Cue)]) -> Song {
        let mut values: Vec<f32> = Arr::new().parameters().iter().map(|p| p.default).collect();
        values[Arr::PARAM_SECTIONS] = lengths.len() as f32;
        for (section, &bars) in lengths.iter().enumerate() {
            values[Arr::PARAM_LENGTH + section] = bars as f32;
        }
        for &(section, lane, cue) in cues {
            values[Arr::cue_param(section, lane)] = cue.encode();
            values[Arr::from_param(section, lane)] = cue.encode_from();
        }
        Song::of(values)
    }

    #[test]
    fn the_timeline_follows_the_cues_as_the_module_does() {
        let song = song(&[2, 4, 2], &[(0, 0, Cue::jump(400)), (1, 0, Cue::ramp(800, 2)), (2, 0, Cue::HOLD)]);
        let starts = song.lane_starts(0);
        let state = |section: usize| starts[section].cued(song.cue(section, 0), song.sections[section].0 as f32, song.sections[section].1 as f32);
        assert_eq!(state(0).at(1.0), 0.4);
        assert!((state(1).at(3.0) - 0.6).abs() < 1e-5, "half way through a two-bar ramp");
        assert_eq!(state(1).at(5.0), 0.8);
        assert_eq!(state(2).at(7.0), 0.8, "held");
    }

    #[test]
    fn a_followers_cues_move_with_the_leaders_sections() {
        use crate::graph::{create_editor_state, SynthNodeTemplate};
        use egui_node_graph2::NodeTemplateTrait;

        let mut editor = create_editor_state();
        let mut user_state = SynthGraphState::new();
        let template = SynthNodeTemplate::from_module_id(arranger_follow::MODULE).unwrap();
        let mut add = |graph: &mut SynthGraph| {
            graph.add_node(template.node_graph_label(&mut user_state), template.user_data(&mut user_state), |graph, node_id| {
                template.build_node(graph, &mut SynthGraphState::new(), node_id)
            })
        };
        let graph = &mut editor.graph;
        let (desk, cues) = (add(graph), add(graph));
        let (output, input) = (graph[desk].get_output("Section").unwrap(), graph[cues].get_input(arranger_follow::FOLLOW).unwrap());
        graph.add_connection(output, input, 0);
        let set = |graph: &mut SynthGraph, node: NodeId, index: usize, value: f32| {
            let id = Params::of(graph, node).unwrap().inputs[index].1;
            graph.inputs[id].value.set_actual_value(value);
        };
        set(graph, desk, Arr::PARAM_SECTIONS, 3.0);
        for (section, level) in [100, 200, 300].into_iter().enumerate() {
            set(graph, cues, Arr::cue_param(section, 0), Cue::jump(level).encode());
        }
        user_state.arranger_leaders = arranger_follow::leaders(graph);
        arranger_follow::sync(graph, &user_state.arranger_leaders);

        // What the app does with the edits, and then each frame
        let apply = |graph: &mut SynthGraph, edits: Vec<SynthResponse>| {
            for edit in edits {
                match edit {
                    SynthResponse::EditParameters { node_id, changes, .. } => {
                        for (name, value) in changes {
                            let id = graph[node_id].get_input(&name).unwrap();
                            graph.inputs[id].value.set_actual_value(value);
                        }
                    }
                    SynthResponse::SetLabels { node_id, labels, .. } => {
                        for (key, text) in labels.into_iter().filter(|(_, text)| !text.is_empty()) {
                            graph[node_id].user_data.labels.insert(key, text);
                        }
                    }
                    _ => {}
                }
            }
            let leaders = arranger_follow::leaders(graph);
            arranger_follow::sync(graph, &leaders);
        };
        let levels = |graph: &SynthGraph| {
            let song = Song::of(Params::of(graph, cues).unwrap().values());
            (0..song.count()).map(|section| song.cue(section, 0).level).collect::<Vec<_>>()
        };
        let on_desk = |graph: &SynthGraph, edit: &dyn Fn(NodeId, &Params, &Song) -> Vec<SynthResponse>| down_the_chain(desk, graph, &user_state, edit);

        let edits = on_desk(graph, &|id, params, song| remove(id, graph, params, song, 1));
        apply(graph, edits);
        assert_eq!(levels(graph), [100, 300]);
        let edits = on_desk(graph, &|id, params, song| swap(id, graph, params, song, 0));
        apply(graph, edits);
        assert_eq!(levels(graph), [300, 100]);
        let edits = on_desk(graph, &|id, params, song| insert(id, graph, params, song, 1, Some(0)));
        apply(graph, edits);
        assert_eq!(levels(graph), [300, 300, 100]);
        let edits = on_desk(graph, &|id, params, song| insert(id, graph, params, song, 0, None));
        apply(graph, edits);
        assert_eq!(levels(graph), [0, 300, 300, 100]);
        // The leader has the same sections, its own cues untouched
        let desk_song = Song::of(Params::of(graph, desk).unwrap().values());
        assert_eq!(desk_song.count(), 4);
        assert!((0..4).all(|section| desk_song.cue(section, 0) == Cue::HOLD));
    }

    #[test]
    fn a_ramp_held_into_the_next_section_carries_on() {
        let song = song(&[1, 1], &[(0, 0, Cue::ramp(1000, 2)), (1, 0, Cue::HOLD)]);
        let starts = song.lane_starts(0);
        let second = starts[1].cued(Cue::HOLD, 1.0, 1.0);
        assert!((second.at(1.0) - 0.5).abs() < 1e-5);
        assert_eq!(second.at(2.0), 1.0);
    }

    #[test]
    fn the_timeline_draws_each_curve_and_start() {
        let song = song(
            &[2, 4, 4, 4],
            &[(0, 0, Cue::jump(400)), (1, 0, Cue::ramp(800, 0).on(Curve::Exp)), (2, 0, Cue::ramp(400, 0).on(Curve::Log)), (3, 0, Cue::ramp(600, 0).on(Curve::Linear).starting(200))],
        );
        let starts = song.lane_starts(0);
        let state = |section: usize| starts[section].cued(song.cue(section, 0), song.sections[section].0 as f32, song.sections[section].1 as f32);
        // Exp is a fifth of the way at half time, Log four fifths
        assert!((state(1).at(4.0) - (0.4 + 0.4 * 0.2)).abs() < 1e-5, "{}", state(1).at(4.0));
        assert!((state(2).at(8.0) - (0.8 - 0.4 * 0.8)).abs() < 1e-5, "{}", state(2).at(8.0));
        // A ramp from a level starts there, wherever the lane had got to
        assert_eq!(state(3).at(10.0), 0.2);
        assert!((state(3).at(12.0) - 0.4).abs() < 1e-5);
        assert_eq!(state(3).at(14.0), 0.6);
    }

    #[test]
    fn a_cue_describes_its_curve_and_start() {
        assert_eq!(describe(Cue::ramp(420, 0)), "ramps to 42% over the section");
        assert_eq!(describe(Cue::ramp(420, 12).on(Curve::Log).starting(280)), "ramps from 28% to 42% over 12 bars, fast then slow");
    }

    #[test]
    fn sections_share_the_width_by_their_bars() {
        let song = song(&[1, 3], &[]);
        let layout = Layout::of(&song, 0.0, 400.0, 1.0);
        assert_eq!(layout.edges.len(), 3);
        assert_eq!(*layout.edges.last().unwrap(), 400.0);
        // Each keeps the floor, and splits the rest 1:3
        let spare = 400.0 - 2.0 * MIN_SECTION;
        assert!((layout.edges[1] - (MIN_SECTION + spare / 4.0)).abs() < 1e-3);
    }
}
