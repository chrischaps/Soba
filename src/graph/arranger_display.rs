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
//!
//! A long song zooms: Ctrl+scroll over the timeline zooms it about the
//! pointer, scroll pans it, and a thin strip over the ruler shows the whole
//! song with the span in view marked, to drag or double-click to fit. While
//! the song plays the view turns the page to keep the playhead in it. The
//! zoom is the editor's, like the section picked: not saved, not an edit.
//! A follower shares its leader's zoom, so their sections stay lined up.

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, CursorIcon, FontId, Id, LayerId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use egui_node_graph2::{InputId, NodeId};

use crate::app::theme;
use crate::modules::arranger::{Arranger as Arr, Cue, Curve, Move, Position, FROM_WHERE_IT_IS, GATE_NAMES, LANES, LANE_NAMES, MAX_BARS, MAX_GLIDE, MAX_REPEATS, SECTIONS};

use super::arranger_follow::{self, NAME_KEY};
use super::steady_text;
use super::{SynthGraph, SynthGraphState, SynthResponse};

/// Height of one output row, in unzoomed points.
const ROW: f32 = 13.0;
/// Room for the lane names, left of the timeline.
const LABEL: f32 = 52.0;
/// Room for the jack labels, right of the timeline.
const JACK_LABEL: f32 = 44.0;
/// The least room the section bar keeps for the picked section's name.
const NAME_ROOM: f32 = 96.0;
/// How tall the controls in the bar under the timeline are.
const BAR: f32 = 16.0;
/// The most a section's length and repeats can say, which their fields in
/// the bar have room for.
const BARS_SAMPLE: &str = "64 bars";
const TIMES_SAMPLE: &str = "×16";
/// The words of the bar's buttons, and the "Move" between its two.
const BAR_WORDS: [&str; 6] = ["+", "Copy", "◀", "Move", "▶", "Delete"];
/// The narrowest a section is drawn, however short.
const MIN_SECTION: f32 = 10.0;
/// The overview strip's height, over the ruler.
const OVERVIEW: f32 = 4.0;
/// How long after a pan or zoom by hand the view leaves the playhead be, in
/// seconds.
const FOLLOW_PAUSE: f64 = 3.0;
/// How far in from the edge the playhead lands when the view turns the
/// page, as a share of the view.
const PAGE_MARGIN: f32 = 0.1;
/// The fewest bars the view zooms in to.
const FEWEST_BARS: f32 = 2.0;
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

    fn value(&self, index: usize) -> f32 {
        self.graph.get_input(self.inputs[index].1).value.actual_value()
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
    /// Each section's first bar and the length of one pass, in bars.
    sections: Vec<(usize, usize)>,
    /// How many times each section plays.
    repeats: Vec<usize>,
    bars: usize,
}

impl Song {
    fn of(values: Vec<f32>) -> Self {
        let mut sections = Vec::new();
        let mut repeats = Vec::new();
        let mut bars = 0;
        for section in 0..Arr::sections(&values) {
            let length = Arr::length(&values, section);
            let times = Arr::repeats(&values, section);
            sections.push((bars, length));
            repeats.push(times);
            bars += length * times;
        }
        Self { steps: Arr::steps(&values), values, sections, repeats, bars }
    }

    /// The node's song as this pass draws it, worked out once a pass
    /// rather than once for each of its rows.
    fn this_pass(ctx: &egui::Context, node_id: NodeId, params: &Params) -> Arc<Self> {
        let pass = ctx.cumulative_pass_nr();
        let id = Id::new((node_id, "arranger-song"));
        if let Some((at, song)) = ctx.data(|data| data.get_temp::<(u64, Arc<Self>)>(id)) {
            if at == pass {
                return song;
            }
        }
        let song = Arc::new(Self::of(params.values()));
        ctx.data_mut(|data| data.insert_temp(id, (pass, song.clone())));
        song
    }

    fn count(&self) -> usize {
        self.sections.len()
    }

    fn cue(&self, section: usize, lane: usize) -> Cue {
        Arr::cue(&self.values, section, lane)
    }

    /// A section's bars over all its passes.
    fn span(&self, section: usize) -> usize {
        self.sections[section].1 * self.repeats[section]
    }

    /// Every pass of every section in the order they play: the section,
    /// which pass, and the bar it starts on.
    fn passes(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        self.sections.iter().enumerate().flat_map(|(section, &(start, length))| (0..self.repeats[section]).map(move |pass| (section, pass, start + pass * length)))
    }

    /// Where a lane is through each pass of [`Song::passes`], once that
    /// pass's cue is given: its level, and the ramp it's on.
    fn lane_passes(&self, lane: usize) -> Vec<LaneState> {
        let mut state = LaneState::default();
        self.passes()
            .map(|(section, _, start)| {
                state = state.cued(self.cue(section, lane), start as f32, self.sections[section].1 as f32);
                state
            })
            .collect()
    }

    /// Where a lane is as each section starts, before its first pass's cue.
    fn lane_starts(&self, lane: usize) -> Vec<LaneState> {
        let mut before = LaneState::default();
        let mut starts = Vec::with_capacity(self.count());
        for ((_, pass, _), state) in self.passes().zip(self.lane_passes(lane)) {
            if pass == 0 {
                starts.push(before);
            }
            before = state;
        }
        starts
    }

    /// How far into a section's block the playhead is, in bars.
    fn playhead(&self, p: &Position) -> f32 {
        let pass = p.pass.min(self.repeats[p.section] - 1);
        (pass * self.sections[p.section].1 + p.bar) as f32 + p.step / self.steps as f32
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
        for section in 0..song.count() {
            x += floor + spare * song.span(section) as f32 / song.bars.max(1) as f32;
            edges.push(x);
        }
        Self { edges }
    }

    /// The x of a point `bar` bars (fractional) into a section, counting
    /// through all its passes.
    fn x(&self, song: &Song, section: usize, bar: f32) -> f32 {
        let (left, right) = (self.edges[section], self.edges[section + 1]);
        left + (right - left) * (bar / song.span(section) as f32).clamp(0.0, 1.0)
    }

    fn span(&self, section: usize) -> (f32, f32) {
        (self.edges[section], self.edges[section + 1])
    }

    /// The bar (from the song's start, fractional) drawn at `x`.
    fn bar_at(&self, song: &Song, x: f32) -> f32 {
        let last = song.count() - 1;
        let section = (0..last).find(|&section| x < self.edges[section + 1]).unwrap_or(last);
        let (left, right) = self.span(section);
        let start = song.sections[section].0;
        start as f32 + song.span(section) as f32 * ((x - left) / (right - left).max(1e-6)).clamp(0.0, 1.0)
    }

    /// The x of a bar (from the song's start, fractional).
    fn x_at(&self, song: &Song, bar: f32) -> f32 {
        let section = song.sections.iter().rposition(|&(start, _)| start as f32 <= bar).unwrap_or(0);
        self.x(song, section, bar - song.sections[section].0 as f32)
    }
}

/// How the timeline is zoomed. It's the editor's, not the patch's, so it
/// isn't saved and isn't an edit, like the section picked.
///
/// A chain of Arrangers that follow one another shares one view, kept for
/// the Arranger at its top, so zooming or panning any of them moves them
/// all and their sections stay lined up (see [`view_owner`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    /// How many timelines wide the whole song is drawn: 1 shows it all.
    scale: f32,
    /// How far into the song the timeline's left edge is, as a share of
    /// the song's drawn width.
    offset: f32,
    /// A scroll still gliding in, as a share of the song's drawn width,
    /// so it slides a chain's timelines alike whatever their widths.
    glide: f32,
    /// The frame the glide last slid in, in egui's time, so a chain's
    /// timelines slide it once between them.
    glided: f64,
    /// When the view was last panned or zoomed by hand, in egui's time.
    touched: f64,
}

impl Default for View {
    fn default() -> Self {
        Self { scale: 1.0, offset: 0.0, glide: 0.0, glided: f64::NEG_INFINITY, touched: f64::NEG_INFINITY }
    }
}

/// What a timeline saw last frame, its own even when its view is shared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Watch {
    /// Where the playhead was, in bars, to tell it's playing.
    playhead: Option<f32>,
    /// The section picked, to bring a new pick into view.
    picked: Option<usize>,
}

impl View {
    /// The most a song is zoomed: until a couple of bars fill the view.
    fn max_scale(song: &Song) -> f32 {
        (song.bars as f32 / FEWEST_BARS).max(1.0)
    }

    fn zoomed(&self) -> bool {
        self.scale > 1.001
    }

    fn clamped(self, song: &Song) -> Self {
        let scale = self.scale.clamp(1.0, Self::max_scale(song));
        Self { scale, offset: self.offset.clamp(0.0, 1.0 - 1.0 / scale), ..self }
    }

    /// Where the sections fall, the whole song drawn `scale` timelines wide
    /// and slid left by the offset.
    fn layout(&self, song: &Song, timeline: Rect, z: f32) -> Layout {
        let width = timeline.width() * self.scale;
        Layout::of(song, timeline.left() - self.offset * width, width, z)
    }

    /// Zoomed by `factor` about `x`, keeping the bar under it in place.
    fn zoomed_about(self, song: &Song, timeline: Rect, z: f32, x: f32, factor: f32) -> Self {
        let bar = self.layout(song, timeline, z).bar_at(song, x);
        let scale = (self.scale * factor).clamp(1.0, Self::max_scale(song));
        let width = timeline.width() * scale;
        let at = Layout::of(song, 0.0, width, z).x_at(song, bar);
        Self { scale, offset: (at - (x - timeline.left())) / width, ..self }.clamped(song)
    }

    /// Slid `by` points later in the song.
    fn panned(self, song: &Song, timeline: Rect, by: f32) -> Self {
        Self { offset: self.offset + by / (timeline.width() * self.scale), ..self }.clamped(song)
    }

    /// Slid so `x` lands a page margin in from the left, if it's out of
    /// view or about to leave it.
    fn turned_to(self, song: &Song, timeline: Rect, x: f32) -> Self {
        let margin = timeline.width() * PAGE_MARGIN;
        if x >= timeline.left() && x <= timeline.right() - margin {
            return self;
        }
        self.panned(song, timeline, x - (timeline.left() + margin))
    }

    /// The span in view, as shares of the whole song's drawn width.
    fn window(&self) -> (f32, f32) {
        (self.offset, self.offset + 1.0 / self.scale)
    }
}

fn view_id(node_id: NodeId) -> Id {
    Id::new((node_id, "arranger-view"))
}

fn watch_id(node_id: NodeId) -> Id {
    Id::new((node_id, "arranger-watch"))
}

/// Whose view an Arranger shows: the one at the top of its chain, or its
/// own if it follows no one.
fn view_owner(user_state: &SynthGraphState, node_id: NodeId) -> NodeId {
    arranger_follow::head(&user_state.arranger_leaders, node_id)
}

fn stored_view(ctx: &egui::Context, owner: NodeId, song: &Song) -> View {
    ctx.data(|data| data.get_temp::<View>(view_id(owner))).unwrap_or_default().clamped(song)
}

/// What the wheel did over a timeline, kept for the timeline to act on.
#[derive(Clone, Copy, Debug)]
struct Wheel {
    /// How far to slide, in points, later in the song.
    pan: f32,
    /// How much to zoom in, as a factor.
    zoom: f32,
    /// Where the pointer was.
    at: Pos2,
}

fn wheel_id(node_id: NodeId) -> Id {
    Id::new((node_id, "arranger-wheel"))
}

/// A timeline drawn last frame: over it, the wheel turns it instead of the
/// canvas.
#[derive(Clone, Copy, Debug)]
struct Claim {
    node_id: NodeId,
    layer: LayerId,
    rect: Rect,
    /// Zoomed in, so a plain scroll pans it. Otherwise only a zoom is its.
    zoomed: bool,
}

fn claims_id() -> Id {
    Id::new("arranger-wheel-claims")
}

/// Takes the wheel's turns over a timeline out of the frame's input before
/// the canvas zooms with them, and keeps them for the timeline: Ctrl+scroll
/// (or a pinch) zooms it, and scroll pans it once it's zoomed in. It goes
/// by where the timelines were drawn last frame. Call it from the app's raw
/// input hook.
pub fn claim_wheel(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(claims) = ctx.data_mut(|data| data.remove_temp::<Vec<Claim>>(claims_id())) else { return };
    if claims.is_empty() {
        return;
    }
    let line = ctx.options(|options| options.input_options.line_scroll_speed);
    let mut pointer = ctx.input(|input| input.pointer.latest_pos());
    let claim_at = |pointer: Option<Pos2>, zoom: bool| {
        let at = pointer?;
        claims.iter().find(|c| c.rect.contains(at) && (zoom || c.zoomed) && ctx.layer_id_at(at) == Some(c.layer)).map(|c| (*c, at))
    };
    let add = |claim: Claim, at: Pos2, pan: f32, zoom: f32| {
        ctx.data_mut(|data| {
            let wheel = data.get_temp_mut_or_insert_with(wheel_id(claim.node_id), || Wheel { pan: 0.0, zoom: 1.0, at });
            wheel.pan += pan;
            wheel.zoom *= zoom;
            wheel.at = at;
        });
    };
    raw.events.retain(|event| match event {
        egui::Event::PointerMoved(at) => {
            pointer = Some(*at);
            true
        }
        egui::Event::MouseWheel { unit, delta, modifiers } => {
            let zoom = modifiers.ctrl || modifiers.command || modifiers.mac_cmd;
            let Some((claim, at)) = claim_at(pointer, zoom) else { return true };
            let delta = match unit {
                egui::MouseWheelUnit::Point => *delta,
                egui::MouseWheelUnit::Line => *delta * line,
                egui::MouseWheelUnit::Page => *delta * claim.rect.width(),
            };
            if zoom {
                add(claim, at, 0.0, (delta.y / 200.0).exp());
            } else {
                // Down or right goes later in the song
                add(claim, at, -(delta.x + delta.y), 1.0);
            }
            false
        }
        egui::Event::Zoom(factor) => {
            let Some((claim, at)) = claim_at(pointer, true) else { return true };
            add(claim, at, 0.0, *factor);
            false
        }
        _ => true,
    });
}

/// Marks a timeline as drawn, for [`claim_wheel`] next frame.
fn claim(ui: &egui::Ui, node_id: NodeId, timeline: Rect, zoomed: bool) {
    let rect = timeline.intersect(ui.clip_rect());
    if rect.is_positive() {
        let claim = Claim { node_id, layer: ui.layer_id(), rect, zoomed };
        ui.ctx().data_mut(|data| data.get_temp_mut_or_default::<Vec<Claim>>(claims_id()).push(claim));
    }
}

/// The overview strip, along the top of the timeline.
fn overview_rect(timeline: Rect, z: f32) -> Rect {
    Rect::from_min_size(timeline.left_top(), Vec2::new(timeline.width(), OVERVIEW * z))
}

/// Works out this frame's view: the wheel's turns, the overview strip
/// dragged, the playhead followed and a new pick brought into view. The
/// song's first row does this, before any row is drawn.
///
/// The view is `owner`'s, which a follower shares with its leader. It's
/// kept under the follower's own name as well, so a follower unpatched
/// carries on from the view it had.
fn update_view(ui: &egui::Ui, node_id: NodeId, owner: NodeId, song: &Song, timeline: Rect, z: f32, song_now: Now) -> View {
    let Now { position, selected } = song_now;
    let ctx = ui.ctx().clone();
    let (now, dt) = ctx.input(|input| (input.time, input.stable_dt.min(0.1)));
    let mut view = stored_view(&ctx, owner, song);
    let mut watch: Watch = ctx.data(|data| data.get_temp(watch_id(node_id))).unwrap_or_default();

    if let Some(wheel) = ctx.data_mut(|data| data.get_temp::<Wheel>(wheel_id(node_id)).inspect(|_| data.remove::<Wheel>(wheel_id(node_id)))) {
        if wheel.zoom != 1.0 {
            view = view.zoomed_about(song, timeline, z, wheel.at.x, wheel.zoom);
        }
        view.glide += wheel.pan / (timeline.width() * view.scale);
        view.touched = now;
    }
    // A scroll glides in, as the canvas's does, once a frame however many
    // timelines share it
    if view.glide != 0.0 && view.glided != now {
        let point = 1.0 / (timeline.width() * view.scale);
        let step = if view.glide.abs() < 0.5 * point { view.glide } else { view.glide * (1.0 - (-18.0 * dt).exp()) };
        view = View { offset: view.offset + step, ..view }.clamped(song);
        view.glide -= step;
        view.glided = now;
        // Nothing left to slide into at the song's ends
        let (left, right) = view.window();
        if (left <= 0.0 && view.glide < 0.0) || (right >= 1.0 && view.glide > 0.0) {
            view.glide = 0.0;
        }
        crate::app::pace::animate(&ctx);
    }

    // The overview strip: click or drag to move the view, double-click to
    // see the whole song
    let strip = overview_rect(timeline, z);
    let response = ui
        .interact(strip.expand2(Vec2::new(0.0, 1.5 * z)).intersect(ui.clip_rect()), Id::new((node_id, "arranger-overview")), Sense::click_and_drag())
        .on_hover_cursor(if view.zoomed() { CursorIcon::Grab } else { CursorIcon::Default })
        .on_hover_text("The whole song, with the part in view marked\nDrag along it to move · double-click to see it all · Ctrl+scroll the timeline to zoom");
    if response.double_clicked() {
        view = View { touched: now, ..View::default() };
    } else if let Some(pointer) = response.interact_pointer_pos() {
        let at = (pointer.x - strip.left()) / strip.width();
        let (left, right) = view.window();
        if (response.drag_started() && (at < left || at > right)) || response.clicked() {
            view.offset = at - (right - left) / 2.0;
        } else if response.dragged() {
            view.offset += response.drag_delta().x / strip.width();
        }
        view.glide = 0.0;
        view.touched = now;
        view = view.clamped(song);
    }

    // While the song plays, the view turns the page to keep the playhead in
    // it, unless it was just moved by hand
    let playhead = position.filter(|p| !p.ended).map(|p| song.sections[p.section].0 as f32 + song.playhead(&p));
    let playing = playhead.is_some() && playhead != watch.playhead;
    watch.playhead = playhead;
    if let Some(bar) = playhead.filter(|_| playing && view.zoomed() && now - view.touched > FOLLOW_PAUSE) {
        let x = view.layout(song, timeline, z).x_at(song, bar);
        view = view.turned_to(song, timeline, x);
    }

    // A section picked from the bar below comes into view
    if watch.picked != Some(selected) {
        if watch.picked.is_some() && view.zoomed() {
            let (x0, x1) = view.layout(song, timeline, z).span(selected);
            if x1 < timeline.left() || x0 > timeline.right() {
                view = view.turned_to(song, timeline, x0);
                view.touched = now;
            }
        }
        watch.picked = Some(selected);
    }

    let view = view.clamped(song);
    // The chain's timelines drawn before this one this frame catch up next
    if view != stored_view(&ctx, owner, song) {
        crate::app::pace::animate(&ctx);
    }
    ctx.data_mut(|data| {
        data.insert_temp(view_id(owner), view);
        data.insert_temp(view_id(node_id), view);
        data.insert_temp(watch_id(node_id), watch);
    });
    view
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
fn lane_name(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId, lane: usize) -> (Option<String>, bool) {
    let Some(node) = graph.nodes.get(node_id) else { return (None, false) };
    if let Some(name) = node.user_data.labels.get(&lane_key(lane)) {
        return (Some(name.clone()), true);
    }
    for port in [LANE_NAMES[lane], GATE_NAMES[lane]] {
        let Ok(output) = node.get_output(port) else { continue };
        if let Some(input) = user_state.cables_from.first(output) {
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
/// and the section playing, with which pass of it if it repeats
/// ("Desk  ▸  Groove 3/4").
pub fn title_note(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId) -> Option<String> {
    let position = position(user_state, node_id).filter(|p| p.started);
    let repeats = |section: usize| Params::of(graph, node_id).map_or(1, |params| Arr::repeat_count(params.value(Arr::PARAM_REPEAT + section)));
    let playing = position.map(|p| match (p.ended, pass_count(&p, repeats(p.section))) {
        (true, _) => "End".to_string(),
        (false, None) => section_name(graph, node_id, p.section),
        (false, Some(count)) => format!("{} {count}", section_name(graph, node_id, p.section)),
    });
    match (arranger_follow::given_name(graph, node_id), playing) {
        (Some(name), Some(playing)) => Some(format!("{name}  ▸  {playing}")),
        (Some(name), None) => Some(name),
        (None, Some(playing)) => Some(format!("▸  {playing}")),
        (None, None) => None,
    }
}

/// "3/4" for the third pass of a section ×4, or None if it plays once.
fn pass_count(position: &Position, repeats: usize) -> Option<String> {
    (repeats > 1).then(|| format!("{}/{repeats}", position.pass.min(repeats - 1) + 1))
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
    let width = Arr::width(params.value(Arr::PARAM_WIDTH));

    let (rect, _) = ui.allocate_exact_size(Vec2::new((LABEL + width + JACK_LABEL) * z, ROW * z), Sense::hover());

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

    let song = Song::this_pass(ui.ctx(), node_id, &params);
    let timeline = Rect::from_min_max(Pos2::new(block.left() + LABEL * z, block.top()), block.max);
    let position = position(user_state, node_id).filter(|p| p.started && p.section < song.count());
    let selected = selected(ui.ctx(), node_id).min(song.count() - 1);
    let now = Now { position, selected };
    // The song's first row works out the view for every row
    let owner = view_owner(user_state, node_id);
    let view = match row {
        Row::Sections => update_view(ui, node_id, owner, &song, timeline, z, now),
        _ => stored_view(ui.ctx(), owner, &song),
    };
    claim(ui, node_id, timeline, view.zoomed());
    let layout = view.layout(&song, timeline, z);
    // Zoomed in, the song runs on past the timeline's ends
    let gap = ui.spacing().item_spacing.y;
    let clip = Rect::from_x_y_ranges(timeline.x_range(), timeline.y_range().expand(gap)).intersect(ui.clip_rect());
    let frame = Frame { block, timeline, clip, view };
    let cx = Ctx { node_id, graph, user_state, params: &params, song: &song };
    let mut edits = Vec::new();
    match row {
        Row::Sections => sections(ui, cx, &layout, &frame, now, &mut edits),
        Row::Lane(lane) => lane_rows(ui, cx, &layout, &frame, lane, now, &mut edits),
        _ => {}
    }

    // The playhead, through every row
    if let Some(p) = position {
        let x = layout.x(&song, p.section, song.playhead(&p));
        let top = if matches!(row, Row::Sections) { timeline.top() + (OVERVIEW + 1.0) * z } else { timeline.top() - gap / 2.0 };
        let span = egui::Rangef::new(top, timeline.bottom() + gap / 2.0);
        let strength = if p.ended { 0.25 } else { 0.8 };
        ui.painter().with_clip_rect(clip).vline(x, span, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(strength)));
    }
    Some(edits)
}

/// The Arranger whose rows are being drawn: its parameters and its song.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    node_id: NodeId,
    graph: &'a SynthGraph,
    user_state: &'a SynthGraphState,
    params: &'a Params<'a>,
    song: &'a Song,
}

/// Where the song is this frame: the section playing, and the one picked.
#[derive(Clone, Copy)]
struct Now {
    position: Option<Position>,
    selected: usize,
}

/// Where a row's part of the timeline is drawn.
#[derive(Clone, Copy)]
struct Frame {
    /// The rows the part covers, from the lane names to the jack labels.
    block: Rect,
    /// The timeline's part of them.
    timeline: Rect,
    /// What of the timeline can be drawn: zoomed in, the song runs on past
    /// its ends.
    clip: Rect,
    view: View,
}

/// The song's rows: the overview strip and a bar ruler, then a block for
/// each section.
fn sections(ui: &mut egui::Ui, cx: Ctx, layout: &Layout, frame: &Frame, now: Now, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, graph, user_state, song, .. } = cx;
    let Now { position, selected } = now;
    let z = user_state.zoom;
    let Frame { block, timeline, clip, view } = *frame;
    let painter = ui.painter().clone();
    let tl = painter.with_clip_rect(clip);
    let green = theme::signal::GATE;

    // Where the song is, at the left: bar of bars
    let font = FontId::proportional(9.0 * z);
    let big = FontId::proportional(15.0 * z);
    let left = block.left() + 2.0 * z;
    let bar_now = position.map(|p| song.sections[p.section].0 + song.playhead(&p) as usize + 1);
    let (number, number_ink) = match bar_now {
        Some(bar) => (bar.to_string(), theme::text::PRIMARY),
        None => ("–".to_string(), theme::text::DISABLED),
    };
    painter.text(Pos2::new(left, block.top() + 2.0 * z), Align2::LEFT_TOP, "BAR", font.clone(), theme::text::QUIET);
    painter.text(Pos2::new(left, block.top() + 12.0 * z), Align2::LEFT_TOP, number, big, number_ink);
    painter.text(Pos2::new(left, block.top() + 30.0 * z), Align2::LEFT_TOP, format!("of {}", song.bars), font.clone(), theme::text::SECONDARY);
    // The section playing, under the bar count
    if let Some(p) = position {
        let width = LABEL * z - 6.0 * z;
        let text = match (p.ended, pass_count(&p, song.repeats[p.section])) {
            (true, _) => "End".to_string(),
            (false, None) => steady_text::fit(painter.ctx(), &section_name(graph, node_id, p.section), &font, width),
            // The pass count always shows; the name gives way to it
            (false, Some(count)) => {
                let count = format!(" {count}");
                let room = width - painter.layout_no_wrap(count.clone(), font.clone(), green).size().x;
                steady_text::fit(painter.ctx(), &section_name(graph, node_id, p.section), &font, room) + &count
            }
        };
        painter.text(Pos2::new(left, block.top() + 44.0 * z), Align2::LEFT_TOP, text, font.clone(), green);
    }

    overview(&tl, song, layout, &view, timeline, z, now);

    // The ruler: a tick a bar if they'd be far enough apart, a taller one
    // between a section's passes, and a number where each section starts,
    // and on the bars between once there's room
    let ruler = Rect::from_min_max(Pos2::new(timeline.left(), timeline.top() + (OVERVIEW + 2.0) * z), Pos2::new(timeline.right(), timeline.top() + (OVERVIEW + 12.0) * z));
    let seen = |x0: f32, x1: f32| x1 >= timeline.left() && x0 <= timeline.right();
    let number_font = FontId::monospace(7.5 * z);
    for (section, &(start, length)) in song.sections.iter().enumerate() {
        let (x0, x1) = layout.span(section);
        if !seen(x0, x1) {
            continue;
        }
        let per_bar = (x1 - x0) / song.span(section) as f32;
        let every = [1, 2, 4, 8, 16].into_iter().find(|&k| per_bar * k as f32 >= 18.0 * z);
        for bar in 1..song.span(section) {
            let between_passes = bar % length == 0;
            let x = x0 + per_bar * bar as f32;
            if seen(x, x) && (per_bar >= 3.0 * z || (between_passes && per_bar * length as f32 >= 3.0 * z)) {
                let tall = if between_passes { 4.5 } else { 2.5 };
                tl.vline(x, egui::Rangef::new(ruler.bottom() - tall * z, ruler.bottom()), Stroke::new(1.0_f32, theme::background::GRID_MAJOR));
                // Clear of the next section's number
                if every.is_some_and(|k| bar % k == 0) && x1 - x >= 16.0 * z {
                    tl.text(Pos2::new(x + 2.0 * z, ruler.top()), Align2::LEFT_TOP, (start + bar + 1).to_string(), number_font.clone(), theme::text::QUIET.gamma_multiply(0.8));
                }
            }
        }
        tl.vline(x0, egui::Rangef::new(ruler.top() + 1.0 * z, ruler.bottom()), Stroke::new(1.0_f32, theme::text::DISABLED));
        if x1 - x0 >= 14.0 * z {
            tl.text(Pos2::new(x0 + 2.0 * z, ruler.top()), Align2::LEFT_TOP, (start + 1).to_string(), number_font.clone(), theme::text::QUIET);
        }
    }

    // The blocks
    let blocks = Rect::from_min_max(Pos2::new(timeline.left(), ruler.bottom() + 1.5 * z), Pos2::new(timeline.right(), timeline.bottom() - 1.0 * z));
    let renaming: Option<Renaming> = ui.ctx().data(|data| data.get_temp(renaming_id(node_id)));
    let name_font = FontId::proportional(10.0 * z);
    // A name too long for its block, shown whole while it's hovered
    let mut hovered_name: Option<(Rect, String)> = None;
    for section in 0..song.count() {
        let (x0, x1) = layout.span(section);
        if !seen(x0, x1) {
            continue;
        }
        let rect = Rect::from_min_max(Pos2::new(x0 + 0.75 * z, blocks.top()), Pos2::new(x1 - 0.75 * z, blocks.bottom()));
        // The part in view, where its name and number go
        let shown = rect.intersect(timeline);
        let repeats = song.repeats[section];
        let length = song.sections[section].1;
        let now = position.filter(|p| p.section == section && !p.ended);
        let playing = now.is_some();
        let queued = position.is_some_and(|p| p.queued == Some(section));
        let round = 3.0 * z;
        let rest = if section % 2 == 0 { theme::background::WIDGET } else { theme::background::WIDGET.lerp_to_gamma(theme::background::PANEL, 0.4) };
        let fill = match now {
            Some(_) if repeats == 1 => green.gamma_multiply(0.30),
            // A repeated section glows faintly, and the pass playing fully
            Some(_) => rest.lerp_to_gamma(green, 0.16),
            None => rest,
        };
        tl.rect_filled(rect, round, fill);
        let pass_x = |pass: usize| layout.x(song, section, (pass * length) as f32);
        if let Some(p) = now.filter(|_| repeats > 1) {
            let pass = p.pass.min(repeats - 1);
            let lit = Rect::from_x_y_ranges(pass_x(pass).max(rect.left())..=pass_x(pass + 1).min(rect.right()), rect.y_range());
            tl.rect_filled(lit, round, green.gamma_multiply(0.24));
        }
        // Each pass's last bar, where a fill goes
        if length > 1 {
            for pass in 0..repeats {
                let x0 = layout.x(song, section, (pass * length + length - 1) as f32);
                let last = Rect::from_x_y_ranges(x0..=pass_x(pass + 1).min(rect.right()), rect.y_range());
                tl.rect_filled(last, if pass + 1 == repeats { round } else { 0.0 }, Color32::BLACK.gamma_multiply(0.18));
            }
        }
        // Faint dashes between the passes
        for pass in 1..repeats {
            let x = pass_x(pass);
            let ends = [Pos2::new(x, rect.top() + 2.0 * z), Pos2::new(x, rect.bottom() - 2.0 * z)];
            tl.extend(Shape::dashed_line(&ends, Stroke::new(1.0 * z, theme::text::SECONDARY.gamma_multiply(0.45)), 2.0 * z, 2.0 * z));
        }
        if playing {
            tl.hline(rect.x_range(), rect.top() + 0.75 * z, Stroke::new(1.5 * z, green));
        }
        if queued {
            tl.rect_stroke(rect, round, Stroke::new(1.0 * z, green.gamma_multiply(0.8)), egui::StrokeKind::Middle);
        }
        if section == selected {
            tl.rect_stroke(rect.expand(0.5 * z), round, Stroke::new(1.2 * z, Color32::WHITE.gamma_multiply(0.75)), egui::StrokeKind::Middle);
        }

        let name = section_name(graph, node_id, section);
        let key = section_key(section);
        let is_renaming = renaming.as_ref().is_some_and(|r| r.key == key);
        let mut cut_short = false;
        // A block mostly slid out of view shows no name, not a lone "…"
        let sliver = shown.width() < 20.0 * z && shown.width() < rect.width() - 1.0;
        if !is_renaming {
            // ×N in the corner of the part in view, counting the passes
            // off while it plays
            let mut badge_width = 0.0;
            if repeats > 1 && !sliver {
                let text = match now {
                    Some(p) => pass_count(&p, repeats).unwrap_or_default(),
                    None => format!("×{repeats}"),
                };
                let badge_font = FontId::monospace(8.0 * z);
                let ink = if playing { Color32::WHITE } else { green.lerp_to_gamma(theme::text::SECONDARY, 0.35) };
                let galley = painter.layout_no_wrap(text, badge_font, ink);
                if galley.size().x + 6.0 * z <= shown.width() {
                    badge_width = galley.size().x + 4.0 * z;
                    tl.galley(Pos2::new(shown.right() - 3.0 * z - galley.size().x, rect.top() + 3.5 * z), galley, ink);
                }
            }
            let text = if sliver { String::new() } else { steady_text::fit(painter.ctx(), &name, &name_font, shown.width() - 4.0 * z - badge_width) };
            cut_short = text != name;
            let ink = if playing { Color32::WHITE } else { theme::text::PRIMARY };
            // Too narrow for any of the name: the badge says enough
            if !(badge_width > 0.0 && text == "…") {
                tl.text(Pos2::new(shown.left() + 3.0 * z, rect.top() + 3.0 * z), Align2::LEFT_TOP, text, name_font.clone(), ink);
            }
            let bars = format!("{length}");
            if shown.width() > 22.0 * z {
                tl.text(Pos2::new(shown.right() - 3.0 * z, rect.bottom() - 2.0 * z), Align2::RIGHT_BOTTOM, bars, FontId::monospace(8.0 * z), theme::text::SECONDARY);
            }
        }
        if !shown.is_positive() {
            continue;
        }

        super::marks::mark(ui.ctx(), node_id, format_args!("section:{}", section + 1), shown);
        let response = ui
            .interact(shown, Id::new((node_id, "arranger-section", section)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if cut_short && response.hovered() {
            hovered_name = Some((shown, name.clone()));
        }
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
        let plays = if repeats > 1 { format!(" ×{repeats}, {} bars in all", song.span(section)) } else { String::new() };
        let response = response.on_hover_text(format!(
            "{name}: {length} bar{}{plays}, from bar {}\n{how}",
            if length == 1 { "" } else { "s" },
            song.sections[section].0 + 1
        ));
        crate::widgets::menu::context_menu(&response, |ui| {
            select(ui.ctx(), node_id, section);
            section_menu(ui, cx, section, edits);
        });

        if is_renaming {
            let field = shown.shrink2(Vec2::new(1.0 * z, 2.0 * z)).with_max_y(rect.top() + 16.0 * z);
            let field = field.with_max_x(field.right().max(field.left() + 60.0 * z));
            if let Some(done) = rename_field(ui, node_id, field, z) {
                edits.extend(done);
            }
        }
    }

    // The hovered name, whole, on a tag just over its block
    if let Some((shown, name)) = hovered_name {
        let galley = painter.layout_no_wrap(name, name_font, theme::text::PRIMARY);
        let pad = Vec2::new(4.0 * z, 1.5 * z);
        let size = galley.size() + 2.0 * pad;
        let x = shown.left().min(block.right() - size.x).max(block.left());
        let tag = Rect::from_min_size(Pos2::new(x, shown.top() - size.y - 1.0 * z), size);
        painter.rect(tag, 3.0 * z, theme::background::PANEL, Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE), egui::StrokeKind::Middle);
        painter.galley(tag.min + pad, galley, theme::text::PRIMARY);
    }
}

/// The overview strip: the whole song in little, the section playing lit,
/// and the part in view framed while zoomed in.
fn overview(painter: &egui::Painter, song: &Song, layout: &Layout, view: &View, timeline: Rect, z: f32, now: Now) {
    let Now { position, selected } = now;
    let strip = overview_rect(timeline, z);
    let (w0, w1) = (layout.edges[0], layout.edges[song.count()]);
    let to_strip = |x: f32| strip.left() + (x - w0) / (w1 - w0).max(1e-6) * strip.width();
    painter.rect_filled(strip, 1.5 * z, theme::background::PANEL.gamma_multiply(0.7));
    for section in 0..song.count() {
        let (x0, x1) = layout.span(section);
        let (a, b) = (to_strip(x0), to_strip(x1));
        let playing = position.is_some_and(|p| p.section == section && !p.ended);
        let ink = if playing {
            theme::signal::GATE.gamma_multiply(0.8)
        } else if section == selected {
            theme::text::SECONDARY
        } else if section % 2 == 0 {
            theme::background::WIDGET_ACTIVE
        } else {
            theme::background::WIDGET
        };
        let inset = if b - a > 2.0 * z { 0.4 * z } else { 0.0 };
        painter.rect_filled(Rect::from_x_y_ranges(a + inset..=b - inset, strip.y_range()), 1.0 * z, ink);
    }
    if let Some(p) = position {
        let x = to_strip(layout.x(song, p.section, song.playhead(&p)));
        painter.vline(x, strip.y_range(), Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(if p.ended { 0.3 } else { 0.9 })));
    }
    if view.zoomed() {
        let (left, right) = view.window();
        let (a, b) = (strip.left() + left * strip.width(), strip.left() + right * strip.width());
        let shade = Color32::BLACK.gamma_multiply(0.45);
        painter.rect_filled(Rect::from_x_y_ranges(strip.left()..=a, strip.y_range()), 0.0, shade);
        painter.rect_filled(Rect::from_x_y_ranges(b..=strip.right(), strip.y_range()), 0.0, shade);
        let window = Rect::from_x_y_ranges(a..=b, strip.y_range()).expand(0.75 * z);
        painter.rect_stroke(window, 1.5 * z, Stroke::new(1.0 * z, Color32::WHITE.gamma_multiply(0.8)), egui::StrokeKind::Middle);
    }
}

// The name field of a section or lane being renamed, over `rect`. Returns
/// the edit once it's typed.
fn rename_field(ui: &mut egui::Ui, node_id: NodeId, rect: Rect, z: f32) -> Option<Vec<SynthResponse>> {
    let id = renaming_id(node_id);
    let mut renaming: Renaming = ui.ctx().data(|data| data.get_temp(id))?;
    let field_id = Id::new((node_id, "arranger-rename-field"));
    let mut output = ui
        .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
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
fn lane_rows(ui: &mut egui::Ui, cx: Ctx, layout: &Layout, frame: &Frame, lane: usize, now: Now, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, graph, user_state, params, song } = cx;
    let Now { position, selected } = now;
    let z = user_state.zoom;
    let Frame { block, timeline, clip, .. } = *frame;
    let painter = ui.painter().clone();
    let orange = theme::signal::CONTROL;
    let green = theme::signal::GATE;

    // The lane's name, given or taken from its cable
    let (name, given) = lane_name(graph, user_state, node_id, lane);
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
        painter.text(Pos2::new(name_rect.left() + 1.0 * z, name_rect.top() + 2.0 * z), Align2::LEFT_TOP, (lane + 1).to_string(), number, theme::text::QUIET);
        let text = steady_text::fit(painter.ctx(), name.as_deref().unwrap_or("–"), &font, name_rect.width() - 2.0 * z);
        let ink = if given { theme::text::PRIMARY } else if name.is_some() { theme::text::SECONDARY } else { theme::text::DISABLED };
        painter.text(Pos2::new(name_rect.left() + 1.0 * z, name_rect.center().y + 3.0 * z), Align2::LEFT_CENTER, text, font, ink);
    }
    let glide = params.value(Arr::PARAM_GLIDE + lane);
    let name_response = ui
        .interact(name_rect, Id::new((node_id, "arranger-lane", lane)), Sense::click())
        .on_hover_ui(|ui| {
            let gliding = if glide > 0.0 { format!(", gliding {}", super::drum_display::duration(glide)) } else { String::new() };
            ui.label(format!("{lane_label}{gliding}\nDouble-click to rename · right-click for its glide"));
        });
    if name_response.double_clicked() {
        let existing = graph.nodes.get(node_id).and_then(|n| n.user_data.labels.get(&key).cloned()).unwrap_or_default();
        ui.ctx().data_mut(|data| data.insert_temp(renaming_id(node_id), Renaming::start(key.clone(), existing)));
    }
    crate::widgets::menu::context_menu(&name_response, |ui| lane_menu(ui, cx, lane, &lane_label, glide, edits));

    // The level's room, and the gate strip under it
    let tl = painter.with_clip_rect(clip);
    let strip = 3.0 * z;
    let area = Rect::from_min_max(Pos2::new(timeline.left(), block.top() + 1.0 * z), Pos2::new(timeline.right(), block.bottom() - strip - 2.0 * z));
    let strip_rect = Rect::from_min_max(Pos2::new(timeline.left(), block.bottom() - strip), Pos2::new(timeline.right(), block.bottom()));
    painter.rect_filled(area, 2.0 * z, theme::background::PANEL.gamma_multiply(0.6));
    painter.rect_filled(strip_rect, 1.5 * z, theme::background::WIDGET.gamma_multiply(0.6));
    let y_of = |level: f32| area.bottom() - level.clamp(0.0, 1.0) * area.height();
    // Zoomed in, only the sections in view are drawn, and only their part in view
    let in_view = |section: usize| {
        let (x0, x1) = layout.span(section);
        (x1 >= timeline.left() && x0 <= timeline.right()).then(|| (x0, x1, x0.max(timeline.left()), x1.min(timeline.right())))
    };

    for section in 0..song.count() {
        let Some((x0, x1, _, _)) = in_view(section) else { continue };
        // The selected section's column, faintly lit
        if section == selected {
            tl.rect_filled(Rect::from_x_y_ranges(x0..=x1, area.y_range()), 0.0, Color32::WHITE.gamma_multiply(0.035));
        }
        // A section boundary, faint, and fainter dashes between its passes
        if section > 0 {
            tl.vline(x0, area.y_range(), Stroke::new(1.0_f32, theme::background::GRID_MAJOR.gamma_multiply(0.7)));
        }
        let length = song.sections[section].1;
        for pass in 1..song.repeats[section] {
            let x = layout.x(song, section, (pass * length) as f32);
            let ends = [Pos2::new(x, area.top()), Pos2::new(x, area.bottom())];
            tl.extend(Shape::dashed_line(&ends, Stroke::new(1.0_f32, theme::background::GRID_MAJOR.gamma_multiply(0.6)), 2.0 * z, 2.0 * z));
        }
    }

    // The level, pass by pass, and the gate strip under it: only the passes
    // in view, and only their part in view
    let passes: Vec<(usize, usize, usize)> = song.passes().collect();
    let states = song.lane_passes(lane);
    let mut points = Vec::new();
    let step = 2.0 * z;
    for (&(section, pass, start), state) in passes.iter().zip(&states) {
        let length = song.sections[section].1;
        let (x0, x1) = (layout.x(song, section, (pass * length) as f32), layout.x(song, section, ((pass + 1) * length) as f32));
        if x1 < timeline.left() || x0 > timeline.right() {
            continue;
        }
        let (from, to) = (x0.max(timeline.left()), x1.min(timeline.right()));
        let samples = (((to - from) / step).ceil() as usize).max(1);
        let mut gate_from: Option<f32> = None;
        for n in 0..=samples {
            let x = from + (to - from) * n as f32 / samples as f32;
            let bar = start as f32 + length as f32 * (x - x0) / (x1 - x0).max(1e-6);
            let level = state.at(bar);
            points.push(Pos2::new(x, y_of(level)));
            match (level > 0.0005, gate_from) {
                (true, None) => gate_from = Some(x),
                (false, Some(from)) => {
                    tl.rect_filled(Rect::from_x_y_ranges(from..=x, strip_rect.y_range()), 1.0 * z, green.gamma_multiply(0.55));
                    gate_from = None;
                }
                _ => {}
            }
        }
        if let Some(from) = gate_from {
            tl.rect_filled(Rect::from_x_y_ranges(from..=to, strip_rect.y_range()), 1.0 * z, green.gamma_multiply(0.55));
        }
    }
    // A lane no section moves is drawn faintly
    let idle = (0..song.count()).all(|section| song.cue(section, lane).kind == Move::Hold);
    let line = if idle { orange.gamma_multiply(0.3) } else { orange };
    if !idle {
        super::drum_display::fill_under(&tl, &points, area.bottom(), orange.gamma_multiply(0.22), orange.gamma_multiply(0.02));
    }
    tl.add(Shape::line(points, Stroke::new(1.4 * z, line)));

    // Hits, as ticks from the strip up to their level, on every pass
    for (section, pass, _) in song.passes() {
        let cue = song.cue(section, lane);
        if cue.kind == Move::Hit {
            let x = layout.x(song, section, (pass * song.sections[section].1) as f32) + 1.5 * z;
            tl.vline(x, egui::Rangef::new(y_of(cue.value()), strip_rect.bottom()), Stroke::new(1.4 * z, green));
            tl.circle_filled(Pos2::new(x, y_of(cue.value())), 2.2 * z, green);
        }
    }

    // Where the lane is now, riding the playhead
    if let Some(p) = position {
        let x = layout.x(song, p.section, song.playhead(&p));
        if let Some(level) = output_value(user_state, node_id, Arr::OUT_LANES + 2 * lane) {
            let c = Pos2::new(x, y_of(level));
            tl.circle_filled(c, 5.0 * z, orange.gamma_multiply(0.25 + 0.3 * level.clamp(0.0, 1.0)));
            tl.circle_filled(c, 2.4 * z, orange.lerp_to_gamma(Color32::WHITE, 0.4));
        }
        if output_value(user_state, node_id, Arr::OUT_LANES + 2 * lane + 1).is_some_and(|v| v > 0.5) {
            tl.circle_filled(Pos2::new(x, strip_rect.center().y), 3.0 * z, green.lerp_to_gamma(Color32::WHITE, 0.4));
        }
    }

    // A cell per section in view: drag to set its level
    for section in 0..song.count() {
        let Some((_, _, from, to)) = in_view(section) else { continue };
        let cell = Rect::from_min_max(Pos2::new(from, block.top()), Pos2::new(to, block.bottom()));
        if !cell.is_positive() {
            continue;
        }
        let cue = song.cue(section, lane);
        let what = format!("{} · {lane_label}", section_name(graph, node_id, section));
        super::marks::mark(ui.ctx(), node_id, format_args!("cell:{}:{}", lane + 1, section + 1), cell);
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
                    let to = if cue.kind == Move::Hold { (song.lane_starts(lane)[section].at(song.sections[section].0 as f32) * 1000.0).round() as u16 } else { cue.level };
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
        crate::widgets::menu::context_menu(&response, |ui| {
            select(ui.ctx(), node_id, section);
            cue_menu(ui, cx, section, lane, cue, &what, edits);
        });
    }
}

/// A cue's right-click menu: how the lane moves, and to what.
fn cue_menu(ui: &mut egui::Ui, cx: Ctx, section: usize, lane: usize, cue: Cue, what: &str, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, params, .. } = cx;
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
    ui.painter().add(Shape::line(points, Stroke::new(1.4_f32, ink)));
    response
}

/// A lane's right-click menu: its glide, and clearing it.
fn lane_menu(ui: &mut egui::Ui, cx: Ctx, lane: usize, lane_label: &str, glide: f32, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, params, song, .. } = cx;
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
        ui.close();
    }
    if ui.button("Hold in every section").on_hover_text("Clears the lane: it stays at 0").clicked() {
        let changes = (0..song.count()).flat_map(|section| cue_changes(params, section, lane, Cue::HOLD)).collect();
        edits.push(edit(node_id, format!("Clear {lane_label}"), changes));
        ui.close();
    }
}

/// A section's right-click menu.
fn section_menu(ui: &mut egui::Ui, cx: Ctx, section: usize, edits: &mut Vec<SynthResponse>) {
    let Ctx { node_id, graph, user_state, .. } = cx;
    let name = section_name(graph, node_id, section);
    ui.label(egui::RichText::new(format!("Section {}: {name}", section + 1)).strong());
    ui.separator();
    match arranger_follow::leader(user_state, node_id) {
        Some(leader) => {
            ui.label(follows_text(graph, leader));
            ui.label(egui::RichText::new(follows_hint(graph, leader)).small().color(theme::text::SECONDARY));
        }
        None => section_controls(ui, cx, section, edits, true),
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
        let gap = 4.0 * zoom;
        ui.style_mut().spacing.item_spacing.x = gap;
        name_field(ui, node_id, graph, LABEL * zoom - gap, zoom, &mut edits);
        let back = bar_slot(ui, "◀", zoom);
        if steady_text::slot(ui, back, egui::Button::new("◀").small()).on_hover_text("The section before").clicked() {
            select(ui.ctx(), node_id, section.saturating_sub(1));
        }
        let on = bar_slot(ui, "▶", zoom);
        if steady_text::slot(ui, on, egui::Button::new("▶").small()).on_hover_text("The section after").clicked() {
            select(ui.ctx(), node_id, (section + 1).min(song.count() - 1));
        }
        let name = section_name(graph, node_id, section);
        let playing = position(user_state, node_id).is_some_and(|p| p.started && !p.ended && p.section == section);
        let ink = if playing { theme::signal::GATE } else { theme::text::PRIMARY };
        let leader = arranger_follow::leader(user_state, node_id);
        let length = song.sections[section].1;
        let repeats = song.repeats[section];
        let times = if repeats > 1 { format!(" ×{repeats}") } else { String::new() };
        let lasts = format!("{length} bar{}{times}", if length == 1 { "" } else { "s" });
        let follows = leader.map(|leader| follows_text(graph, leader));
        // A long name is cut short, to a set room or to what the row leaves
        // after the controls, so picking it doesn't widen the node. All of
        // it is measured as if fonts scaled smoothly, so the bar keeps its
        // width as the view zooms (#255)
        let row = (LABEL + Arr::width(params.value(Arr::PARAM_WIDTH)) + JACK_LABEL) * zoom;
        let controls: f32 = match &follows {
            Some(follows) => gap + bar_slot(ui, &lasts, zoom).x + 6.0 * zoom + gap + bar_slot(ui, follows.text(), zoom).x,
            None => [BARS_SAMPLE, TIMES_SAMPLE].iter().chain(BAR_WORDS.iter()).map(|text| gap + bar_slot(ui, text, zoom).x).sum(),
        };
        let most = (row - ui.min_rect().width() - gap - controls).max(NAME_ROOM * zoom);
        let label = format!("{} · {name}", section + 1);
        let room = bar_slot(ui, &label, zoom).x.min(most);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(room, BAR * zoom), Sense::hover());
        let font = egui::TextStyle::Body.resolve(ui.style());
        let shown = steady_text::fit(ui.ctx(), &label, &font, room);
        ui.painter().text(Pos2::new(rect.left(), rect.center().y), Align2::LEFT_CENTER, shown, font, ink);
        response.on_hover_text(&name);
        match leader.zip(follows) {
            Some((leader, follows)) => {
                bar_text(ui, egui::RichText::new(lasts).color(theme::text::SECONDARY), zoom);
                ui.add_space(6.0 * zoom);
                bar_text(ui, follows, zoom).on_hover_text(follows_hint(graph, leader));
            }
            None => section_controls(ui, Ctx { node_id, graph, user_state, params: &params, song: &song }, section, &mut edits, false),
        }
    });
    edits
}

/// A control's slot in the bar under the timeline: as wide as `text` is
/// if fonts scaled smoothly, with a little to spare for how it's drawn.
fn bar_slot(ui: &egui::Ui, text: &str, z: f32) -> Vec2 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    Vec2::new(steady_text::text_width(ui, text, &font) * 1.08 + 6.0 * z, BAR * z)
}

/// Words in the bar, in a slot of their own.
fn bar_text(ui: &mut egui::Ui, text: egui::RichText, z: f32) -> egui::Response {
    let size = bar_slot(ui, text.text(), z);
    let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Truncate), size.x, egui::TextStyle::Body);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let at = Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, ui.visuals().text_color());
    response
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
        Some(name) => (steady_text::fit(ui.ctx(), name, &font, rect.width() - 2.0 * z), theme::text::PRIMARY),
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
fn section_controls(ui: &mut egui::Ui, cx: Ctx, section: usize, edits: &mut Vec<SynthResponse>, menu: bool) {
    let Ctx { node_id, graph, user_state, params, song } = cx;
    let name = section_name(graph, node_id, section);
    let length = song.sections[section].1;
    let mut bars = length as u32;
    let suffix = if length == 1 { " bar" } else { " bars" };
    let z = user_state.zoom;
    let drag = egui::DragValue::new(&mut bars).range(1..=MAX_BARS as u32).suffix(suffix).speed(0.1);
    let drag = if menu { ui.add(drag) } else { steady_text::slot(ui, bar_slot(ui, BARS_SAMPLE, z), drag) };
    if drag.on_hover_text("How many bars the section lasts").changed() {
        edits.push(edit(node_id, format!("{name} lasts {bars} bars"), vec![(params.name(Arr::PARAM_LENGTH + section), bars as f32)]));
    }
    let repeats = song.repeats[section];
    let mut times = repeats as u32;
    let drag = egui::DragValue::new(&mut times).range(1..=MAX_REPEATS as u32).prefix("×").speed(0.05);
    // As narrow as "×16", not a whole field's width
    let drag = if menu {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.x = ui.spacing().interact_size.y;
            ui.add(drag)
        })
        .inner
    } else {
        steady_text::slot(ui, bar_slot(ui, TIMES_SAMPLE, z), drag)
    };
    if drag.on_hover_text("How many times the section plays before the next: ×4 plays just as four copies of it would").changed() {
        edits.push(edit(node_id, format!("{name} plays ×{times}"), vec![(params.name(Arr::PARAM_REPEAT + section), times as f32)]));
    }
    let count = song.count();
    let full = count >= SECTIONS;
    let mut action = |ui: &mut egui::Ui, text: &str, hint: &str, enabled: bool, edit: &dyn Fn() -> Option<Vec<SynthResponse>>| {
        let button = if menu {
            ui.add_enabled(enabled, egui::Button::new(text))
        } else {
            let size = bar_slot(ui, text, z);
            ui.add_enabled_ui(enabled, |ui| steady_text::slot(ui, size, egui::Button::new(text).small())).inner
        };
        if button.on_hover_text(hint).on_disabled_hover_text(hint).clicked() {
            if let Some(more) = edit() {
                edits.extend(more);
            }
            if menu {
                ui.close();
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
    // In the bar the two moves share one word, "◀ Move ▶", to leave room
    // for the section's name
    let (back, on) = if menu { ("◀ Move", "Move ▶") } else { ("◀", "▶") };
    action(ui, back, "Swap with the section before", section > 0, &|| {
        select(&ctx, node_id, section - 1);
        chain(&|id, params, song| swap(id, graph, params, song, section - 1))
    });
    if !menu {
        bar_text(ui, egui::RichText::new("Move").color(theme::text::SECONDARY), z);
    }
    action(ui, on, "Swap with the section after", section + 1 < count, &|| {
        select(&ctx, node_id, section + 1);
        chain(&|id, params, song| swap(id, graph, params, song, section))
    });
    action(ui, "Delete", "Take this section out of the song", count > 1, &|| {
        select(&ctx, node_id, section.saturating_sub(1).min(count - 2));
        chain(&|id, params, song| remove(id, graph, params, song, section))
    });
    if !menu {
        return;
    }
    // Repeats and copies, either way round: they play the same
    ui.separator();
    let labels = labels_of(graph, node_id);
    let split_hint = if repeats < 2 {
        "Plays once already".to_string()
    } else if count + repeats - 1 > SECTIONS {
        format!("No room: a song has at most {SECTIONS} sections")
    } else {
        format!("Make this section ×{repeats} into {repeats} sections, each ×1, to change one pass")
    };
    // Followers split and merge too, each with its own cues
    action(ui, "Split into copies", &split_hint, split_plan(&labels, song, section).is_some(), &|| {
        let label = format!("Split {name} into {repeats} copies");
        chain(&|id, params, song| split_plan(&labels_of(graph, id), song, section).map_or_else(Vec::new, |rewrite| rewrite.edits(id, params, label.clone())))
    });
    let own = mergeable(&labels, song, section);
    let folds = chain_mergeable(node_id, graph, user_state, section);
    let total: usize = (section..=section + folds).map(|s| song.repeats[s]).sum();
    let merge_hint = if own == 0 {
        "The section after this one plays something else".to_string()
    } else if folds == 0 {
        "An Arranger following this one plays something else in the section after it".to_string()
    } else {
        let copies = if folds == 1 { "the copy".to_string() } else { format!("the {folds} copies") };
        format!("Fold {copies} straight after this section into it: {name} ×{total}")
    };
    action(ui, "Merge with copies after it", &merge_hint, folds > 0, &|| {
        let label = format!("Merge {name} ×{total}");
        chain(&|id, params, song| merge_plan(&labels_of(graph, id), song, section, folds).map_or_else(Vec::new, |rewrite| rewrite.edits(id, params, label.clone())))
    });
}

/// How many sections after `at` can fold into it on this Arranger and on
/// every one down the chain that follows it: on all of them, they have to
/// play the same.
fn chain_mergeable(node_id: NodeId, graph: &SynthGraph, user_state: &SynthGraphState, at: usize) -> usize {
    std::iter::once(node_id)
        .chain(arranger_follow::followers(&user_state.arranger_leaders, node_id))
        .filter_map(|id| Params::of(graph, id).map(|params| mergeable(&labels_of(graph, id), &Song::of(params.values()), at)))
        .min()
        .unwrap_or(0)
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

/// What one section holds: its length, repeats, cues and name.
#[derive(Clone, Debug, PartialEq)]
struct SectionData {
    length: f32,
    repeats: f32,
    cues: [f32; LANES],
    froms: [f32; LANES],
    name: String,
}

impl SectionData {
    /// A new section: four bars that hold every lane.
    fn blank() -> Self {
        Self { length: 4.0, repeats: 1.0, cues: [Cue::HOLD.encode(); LANES], froms: [FROM_WHERE_IT_IS; LANES], name: String::new() }
    }

    /// Plays the same as `other`: the same bars and cues, whatever its name
    /// and repeats.
    fn same_music(&self, other: &Self) -> bool {
        self.length.round() == other.length.round()
            && (0..LANES).all(|lane| Cue::decode(self.cues[lane], self.froms[lane]) == Cue::decode(other.cues[lane], other.froms[lane]))
    }
}

type Labels = std::collections::BTreeMap<String, String>;

fn labels_of(graph: &SynthGraph, node_id: NodeId) -> Labels {
    graph.nodes.get(node_id).map(|n| n.user_data.labels.clone()).unwrap_or_default()
}

fn section_data(labels: &Labels, song: &Song, section: usize) -> SectionData {
    if section >= song.count() {
        return SectionData::blank();
    }
    SectionData {
        length: song.values[Arr::PARAM_LENGTH + section],
        repeats: song.repeats[section] as f32,
        cues: std::array::from_fn(|lane| song.values[Arr::cue_param(section, lane)]),
        froms: std::array::from_fn(|lane| song.values[Arr::from_param(section, lane)]),
        name: labels.get(&section_key(section)).cloned().unwrap_or_default(),
    }
}

/// The song from section `from` on, laid out again, with its new section
/// count and Loop.
struct Rewrite {
    from: usize,
    sections: Vec<SectionData>,
    count: usize,
    loop_to: f32,
}

impl Rewrite {
    /// The parameters it sets, by index.
    fn changes(&self) -> Vec<(usize, f32)> {
        let mut changes = vec![(Arr::PARAM_SECTIONS, self.count as f32), (Arr::PARAM_LOOP, self.loop_to)];
        for (section, data) in self.placed() {
            changes.push((Arr::PARAM_LENGTH + section, data.length));
            changes.push((Arr::PARAM_REPEAT + section, data.repeats));
            for lane in 0..LANES {
                changes.push((Arr::cue_param(section, lane), data.cues[lane]));
                changes.push((Arr::from_param(section, lane), data.froms[lane]));
            }
        }
        changes
    }

    /// The section names it sets, empty to clear one.
    fn labels(&self) -> Vec<(String, String)> {
        self.placed().map(|(section, data)| (section_key(section), data.name.clone())).collect()
    }

    fn placed(&self) -> impl Iterator<Item = (usize, &SectionData)> {
        self.sections.iter().enumerate().map(|(offset, data)| (self.from + offset, data)).take_while(|&(section, _)| section < SECTIONS)
    }

    /// The edits that make it, as one undo step.
    fn edits(&self, node_id: NodeId, params: &Params, label: String) -> Vec<SynthResponse> {
        let changes = self.changes().into_iter().map(|(index, value)| (params.name(index), value)).collect();
        vec![edit(node_id, label.clone(), changes), SynthResponse::SetLabels { node_id, label, labels: self.labels() }]
    }
}

/// Adds a section at `at`: a copy of `copy`, or a blank one.
fn insert(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize, copy: Option<usize>) -> Vec<SynthResponse> {
    let count = song.count();
    let labels = labels_of(graph, node_id);
    let mut new = match copy {
        Some(section) => section_data(&labels, song, section),
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
    moved.extend((at..count).map(|section| section_data(&labels, song, section)));
    // Loop follows the section it named
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize > at { loop_to + 1.0 } else { loop_to };
    Rewrite { from: at, sections: moved, count: count + 1, loop_to }.edits(node_id, params, label)
}

/// Takes section `at` out.
fn remove(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize) -> Vec<SynthResponse> {
    let count = song.count();
    let labels = labels_of(graph, node_id);
    let label = format!("Delete section {}", section_name(graph, node_id, at));
    let mut moved: Vec<SectionData> = (at + 1..count).map(|section| section_data(&labels, song, section)).collect();
    // The emptied slot at the end goes back to a blank section
    moved.push(SectionData::blank());
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize - 1 > at { loop_to - 1.0 } else { loop_to.min((count - 1) as f32) };
    Rewrite { from: at, sections: moved, count: count - 1, loop_to }.edits(node_id, params, label)
}

/// Swaps section `at` with the one after it.
fn swap(node_id: NodeId, graph: &SynthGraph, params: &Params, song: &Song, at: usize) -> Vec<SynthResponse> {
    let labels = labels_of(graph, node_id);
    let label = format!("Move section {}", section_name(graph, node_id, at));
    let moved = vec![section_data(&labels, song, at + 1), section_data(&labels, song, at)];
    Rewrite { from: at, sections: moved, count: song.count(), loop_to: song.values[Arr::PARAM_LOOP] }.edits(node_id, params, label)
}

/// Section `at` ×N as N copies of it, each ×1, if there's room for them.
/// The copies after the first are named "Groove 2", "Groove 3" and so on.
fn split_plan(labels: &Labels, song: &Song, at: usize) -> Option<Rewrite> {
    let count = song.count();
    let repeats = song.repeats[at];
    if repeats < 2 || count + repeats - 1 > SECTIONS {
        return None;
    }
    let data = section_data(labels, song, at);
    let mut moved: Vec<SectionData> = (0..repeats)
        .map(|pass| SectionData {
            repeats: 1.0,
            name: if pass == 0 || data.name.is_empty() { data.name.clone() } else { format!("{} {}", data.name, pass + 1) },
            ..data.clone()
        })
        .collect();
    moved.extend((at + 1..count).map(|section| section_data(labels, song, section)));
    // Loop to this section goes to its first copy, and to a later one follows it
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize - 1 > at { loop_to + (repeats - 1) as f32 } else { loop_to };
    Some(Rewrite { from: at, sections: moved, count: count + repeats - 1, loop_to })
}

/// How many of the sections straight after `at` play the same as it and
/// can fold into it: up to ×16 in all, and not past one Loop goes back to.
fn mergeable(labels: &Labels, song: &Song, at: usize) -> usize {
    let first = section_data(labels, song, at);
    let loop_to = Arr::loop_to(&song.values);
    let mut total = song.repeats[at];
    let mut folds = 0;
    for next in at + 1..song.count() {
        if !first.same_music(&section_data(labels, song, next)) || total + song.repeats[next] > MAX_REPEATS || loop_to == Some(next) {
            break;
        }
        total += song.repeats[next];
        folds += 1;
    }
    folds
}

/// Section `at` and up to `most` copies of it straight after it as one
/// section ×N, keeping the first one's name. None if nothing follows it to
/// fold in.
fn merge_plan(labels: &Labels, song: &Song, at: usize, most: usize) -> Option<Rewrite> {
    let folds = mergeable(labels, song, at).min(most);
    if folds == 0 {
        return None;
    }
    let count = song.count();
    let mut first = section_data(labels, song, at);
    first.repeats = (at..=at + folds).map(|section| song.repeats[section]).sum::<usize>() as f32;
    let mut moved = vec![first];
    moved.extend((at + folds + 1..count).map(|section| section_data(labels, song, section)));
    // The emptied slots at the end go back to blank sections
    moved.extend((0..folds).map(|_| SectionData::blank()));
    let loop_to = song.values[Arr::PARAM_LOOP];
    let loop_to = if loop_to >= 1.0 && loop_to as usize - 1 > at + folds { loop_to - folds as f32 } else { loop_to };
    Some(Rewrite { from: at, sections: moved, count: count - folds, loop_to })
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

    fn state(song: &Song, lane: usize, section: usize, pass: usize) -> LaneState {
        let index = song.passes().position(|(s, p, _)| (s, p) == (section, pass)).unwrap();
        song.lane_passes(lane)[index]
    }

    #[test]
    fn the_timeline_follows_the_cues_as_the_module_does() {
        let song = song(&[2, 4, 2], &[(0, 0, Cue::jump(400)), (1, 0, Cue::ramp(800, 2)), (2, 0, Cue::HOLD)]);
        assert_eq!(state(&song, 0, 0, 0).at(1.0), 0.4);
        assert!((state(&song, 0, 1, 0).at(3.0) - 0.6).abs() < 1e-5, "half way through a two-bar ramp");
        assert_eq!(state(&song, 0, 1, 0).at(5.0), 0.8);
        assert_eq!(state(&song, 0, 2, 0).at(7.0), 0.8, "held");
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

        // Repeats are the leader's too
        let repeats = |graph: &SynthGraph| Song::of(Params::of(graph, cues).unwrap().values()).repeats;
        let edits = vec![edit(desk, "Repeat", vec![(Params::of(graph, desk).unwrap().name(Arr::PARAM_REPEAT + 1), 2.0)])];
        apply(graph, edits);
        assert_eq!(repeats(graph)[..4], [1, 2, 1, 1]);

        // Merge and Split go down the chain, the follower's cues with them
        assert_eq!(chain_mergeable(desk, graph, &user_state, 1), 1);
        let edits = on_desk(graph, &|id, params, song| merge_plan(&labels_of(graph, id), song, 1, 1).unwrap().edits(id, params, "Merge".into()));
        apply(graph, edits);
        assert_eq!(levels(graph), [0, 300, 100]);
        assert_eq!(repeats(graph)[..3], [1, 3, 1]);
        let edits = on_desk(graph, &|id, params, song| split_plan(&labels_of(graph, id), song, 1).unwrap().edits(id, params, "Split".into()));
        apply(graph, edits);
        assert_eq!(levels(graph), [0, 300, 300, 300, 100]);
        assert_eq!(repeats(graph)[..5], [1, 1, 1, 1, 1]);

        // A follower that plays the copies differently keeps them apart
        set(graph, cues, Arr::cue_param(2, 0), Cue::jump(500).encode());
        let desk_song = Song::of(Params::of(graph, desk).unwrap().values());
        assert_eq!(mergeable(&labels_of(graph, desk), &desk_song, 1), 3, "the leader alone would fold the rest");
        assert_eq!(chain_mergeable(desk, graph, &user_state, 1), 0);
    }

    #[test]
    fn a_ramp_held_into_the_next_section_carries_on() {
        let song = song(&[1, 1], &[(0, 0, Cue::ramp(1000, 2)), (1, 0, Cue::HOLD)]);
        let second = state(&song, 0, 1, 0);
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
    fn each_pass_of_a_repeated_section_cues_again() {
        // Two bars ×3, ramping to full over three bars: every pass starts
        // the ramp again from where the last one left it
        let mut song = song(&[2, 1], &[(0, 0, Cue::ramp(1000, 3))]);
        song.values[Arr::PARAM_REPEAT] = 3.0;
        let song = Song::of(song.values);
        assert_eq!(song.bars, 7);
        assert_eq!(song.passes().collect::<Vec<_>>(), [(0, 0, 0), (0, 1, 2), (0, 2, 4), (1, 0, 6)]);
        let first = state(&song, 0, 0, 0);
        let second = state(&song, 0, 0, 1);
        assert!((second.at(2.0) - first.at(2.0)).abs() < 1e-6, "picks up where it was");
        assert!(second.at(4.0) < 1.0, "a fresh three-bar ramp, not the old one's end");
        assert_eq!(state(&song, 0, 0, 2).at(7.0), 1.0);
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

    /// From One Sine's shape in little: a long song with 1-bar sections in it.
    fn long_song() -> Song {
        song(&[4, 4, 4, 4, 8, 1, 16, 32, 1, 16, 32, 1, 4, 1], &[])
    }

    fn timeline() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(440.0, 80.0))
    }

    #[test]
    fn a_bar_and_its_x_are_each_others_inverse() {
        let song = long_song();
        let layout = Layout::of(&song, 100.0, 1760.0, 1.0);
        for bar in [0.0, 3.5, 20.0, 24.25, 60.0, 127.9] {
            assert!((layout.bar_at(&song, layout.x_at(&song, bar)) - bar).abs() < 1e-3, "{bar}");
        }
    }

    #[test]
    fn zooming_keeps_the_bar_under_the_pointer() {
        let (song, timeline) = (long_song(), timeline());
        let view = View::default();
        let x = 300.0;
        let bar = view.layout(&song, timeline, 1.0).bar_at(&song, x);
        let zoomed = view.zoomed_about(&song, timeline, 1.0, x, 6.0);
        assert_eq!(zoomed.scale, 6.0);
        assert!((zoomed.layout(&song, timeline, 1.0).bar_at(&song, x) - bar).abs() < 1e-2);
        // And out again, all the way, shows the whole song
        let out = zoomed.zoomed_about(&song, timeline, 1.0, 450.0, 0.01);
        assert_eq!((out.scale, out.offset), (1.0, 0.0));
    }

    #[test]
    fn zoomed_in_a_one_bar_fill_is_wide_enough_to_name_and_drag() {
        let (song, timeline) = (long_song(), timeline());
        let fill = 5;
        let narrow = View::default().layout(&song, timeline, 1.0).span(fill);
        assert!(narrow.1 - narrow.0 < 15.0);
        let view = View { scale: 8.0, ..View::default() }.clamped(&song);
        let (x0, x1) = view.layout(&song, timeline, 1.0).span(fill);
        assert!(x1 - x0 > 30.0, "{}", x1 - x0);
    }

    #[test]
    fn the_view_stays_on_the_song() {
        let short = song(&[1], &[]);
        let (song, timeline) = (long_song(), timeline());
        let view = View { scale: 4.0, ..View::default() };
        assert_eq!(view.panned(&song, timeline, -500.0).offset, 0.0);
        assert_eq!(view.panned(&song, timeline, 1e6).window().1, 1.0);
        // No closer than a couple of bars
        assert_eq!(View { scale: 1e6, ..view }.clamped(&song).scale, song.bars as f32 / FEWEST_BARS);
        // A short song doesn't zoom at all
        assert_eq!(View { scale: 3.0, ..view }.clamped(&short).scale, 1.0);
    }

    #[test]
    fn the_page_turns_when_the_playhead_nears_the_edge() {
        let (song, timeline) = (long_song(), timeline());
        let view = View { scale: 4.0, ..View::default() };
        // Well inside: no turn
        assert_eq!(view.turned_to(&song, timeline, 300.0), view);
        // Near the right edge: it lands a margin in from the left
        let turned = view.turned_to(&song, timeline, 520.0);
        let moved = (turned.offset - view.offset) * timeline.width() * view.scale;
        assert!((moved - (520.0 - (timeline.left() + timeline.width() * PAGE_MARGIN))).abs() < 1e-2);
    }

    /// Three Arrangers, Desk leading Cues leading More.
    fn chain() -> (SynthGraphState, [NodeId; 3]) {
        use crate::graph::{create_editor_state, SynthNodeTemplate};
        use egui_node_graph2::NodeTemplateTrait;

        let mut editor = create_editor_state();
        let mut user_state = SynthGraphState::new();
        let template = SynthNodeTemplate::from_module_id(arranger_follow::MODULE).unwrap();
        let nodes = [(); 3].map(|_| {
            editor.graph.add_node(template.node_graph_label(&mut user_state), template.user_data(&mut user_state), |graph, node_id| {
                template.build_node(graph, &mut SynthGraphState::new(), node_id)
            })
        });
        user_state.arranger_leaders = [(nodes[1], nodes[0]), (nodes[2], nodes[1])].into();
        (user_state, nodes)
    }

    /// Works out each timeline's view for one frame at `time`, as the song's
    /// first rows do, in the order given.
    fn frame(ctx: &egui::Context, time: f64, user_state: &SynthGraphState, song: &Song, nodes: &[NodeId]) -> Vec<View> {
        let mut views = Vec::new();
        let input = egui::RawInput { time: Some(time), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                views = nodes.iter().map(|&node| update_view(ui, node, view_owner(user_state, node), song, timeline(), 1.0, Now { position: None, selected: 0 })).collect();
            });
        });
        views
    }

    fn turn_wheel(ctx: &egui::Context, node: NodeId, wheel: Wheel) {
        ctx.data_mut(|data| data.insert_temp(wheel_id(node), wheel));
    }

    #[test]
    fn a_chain_of_followers_shares_one_view() {
        let (user_state, [desk, cues, more]) = chain();
        assert_eq!([desk, cues, more].map(|node| view_owner(&user_state, node)), [desk; 3]);
        let (ctx, song) = (egui::Context::default(), long_song());

        // Zoomed on the last of the chain, the whole chain zooms
        turn_wheel(&ctx, more, Wheel { pan: 0.0, zoom: 4.0, at: Pos2::new(300.0, 10.0) });
        frame(&ctx, 1.0, &user_state, &song, &[desk, cues, more]);
        let views = frame(&ctx, 1.1, &user_state, &song, &[desk, cues, more]);
        assert_eq!(views[0].scale, 4.0);
        assert!(views.iter().all(|view| *view == views[0]));

        // Scrolled on the leader, a pan slides them all, and only once a
        // frame however many timelines share it
        let alone = egui::Context::default();
        frame(&alone, 1.1, &SynthGraphState::new(), &song, &[desk]);
        for ctx in [&ctx, &alone] {
            ctx.data_mut(|data| data.insert_temp(view_id(desk), views[0]));
            turn_wheel(ctx, desk, Wheel { pan: 200.0, zoom: 1.0, at: Pos2::new(300.0, 10.0) });
        }
        let shared = frame(&ctx, 2.0, &user_state, &song, &[desk, cues, more]);
        let single = frame(&alone, 2.0, &SynthGraphState::new(), &song, &[desk]);
        assert!(shared[0].offset > views[0].offset);
        assert_eq!(shared[2].offset, single[0].offset);
    }

    #[test]
    fn a_follower_unpatched_keeps_its_view() {
        let (mut user_state, [desk, cues, more]) = chain();
        let (ctx, song) = (egui::Context::default(), long_song());
        turn_wheel(&ctx, desk, Wheel { pan: 0.0, zoom: 3.0, at: Pos2::new(400.0, 10.0) });
        let shared = frame(&ctx, 1.0, &user_state, &song, &[desk, cues, more])[0];
        assert!(shared.zoomed());

        // Cues lets go of Desk, and More follows Cues now
        user_state.arranger_leaders.remove(&cues);
        assert_eq!(view_owner(&user_state, more), cues);
        let views = frame(&ctx, 2.0, &user_state, &song, &[desk, cues, more]);
        assert!(views.iter().all(|view| (view.scale, view.offset) == (shared.scale, shared.offset)));

        // Each zooms on its own now: Desk out, Cues and More stay
        ctx.data_mut(|data| data.insert_temp(view_id(desk), View::default()));
        let views = frame(&ctx, 3.0, &user_state, &song, &[desk, cues, more]);
        assert!(!views[0].zoomed());
        assert_eq!(views[1].scale, shared.scale);
        assert_eq!(views[2], views[1]);
    }

    #[test]
    fn a_chain_that_comes_back_round_has_no_head() {
        let (mut user_state, [desk, cues, more]) = chain();
        user_state.arranger_leaders.insert(desk, more);
        assert_eq!(view_owner(&user_state, desk), desk);
        assert_eq!(view_owner(&user_state, cues), cues);
    }

    #[test]
    fn a_repeated_section_is_as_wide_as_all_its_passes() {
        let mut values = song(&[1, 1], &[]).values;
        values[Arr::PARAM_REPEAT] = 3.0;
        let song = Song::of(values);
        let layout = Layout::of(&song, 0.0, 400.0, 1.0);
        let spare = 400.0 - 2.0 * MIN_SECTION;
        assert!((layout.edges[1] - (MIN_SECTION + spare * 3.0 / 4.0)).abs() < 1e-3);
        // The playhead counts through the passes
        let at = Position { section: 0, pass: 2, bar: 0, step: 8.0, started: true, ..Default::default() };
        assert_eq!(song.playhead(&at), 2.5);
        // And zooming finds its bars through all of them
        for bar in [0.5, 1.5, 2.5, 3.5] {
            assert!((layout.bar_at(&song, layout.x_at(&song, bar)) - bar).abs() < 1e-3, "{bar}");
        }
    }

    /// The song once a rewrite's changes are made.
    fn rewritten(song: &Song, labels: &Labels, rewrite: &Rewrite) -> (Song, Labels) {
        let mut values = song.values.clone();
        for (index, value) in rewrite.changes() {
            values[index] = value;
        }
        let mut labels = labels.clone();
        for (key, text) in rewrite.labels() {
            if text.is_empty() {
                labels.remove(&key);
            } else {
                labels.insert(key, text);
            }
        }
        (Song::of(values), labels)
    }

    /// Every output but Section, which counts sections, over 2.5 times
    /// round the song.
    fn render(song: &Song) -> Vec<Vec<f32>> {
        let ms = 10 * song.steps * song.bars * 5 / 2;
        let mut clock = crate::dsp::SignalBuffer::gate(ms);
        for t in (0..ms).step_by(10) {
            clock.samples[t] = 1.0;
        }
        let mut arranger = Arr::new();
        arranger.prepare(1000.0, ms);
        let mut outputs: Vec<_> = (0..Arr::OUT_COUNT).map(|_| crate::dsp::SignalBuffer::control(ms)).collect();
        let context = crate::dsp::context::ProcessContext::new(1000.0, ms);
        arranger.process(&[&clock], &mut outputs, &song.values, &context);
        outputs.into_iter().enumerate().filter(|&(index, _)| index != Arr::OUT_SECTION).map(|(_, b)| b.samples).collect()
    }

    /// Intro, Groove ×3, Outro, looping back to the Groove.
    fn groove_song() -> (Song, Labels) {
        let mut values = song(
            &[1, 2, 1],
            &[(0, 0, Cue::jump(200)), (1, 0, Cue::ramp(900, 3)), (1, 1, Cue::hit(600)), (2, 0, Cue::ramp(0, 0)), (2, 2, Cue::jump(1000))],
        )
        .values;
        values[Arr::PARAM_STEPS] = 4.0;
        values[Arr::PARAM_REPEAT + 1] = 3.0;
        values[Arr::PARAM_LOOP] = 2.0;
        let labels = Labels::from([(section_key(0), "Intro".to_string()), (section_key(1), "Groove".to_string()), (section_key(2), "Outro".to_string())]);
        (Song::of(values), labels)
    }

    #[test]
    fn split_makes_copies_that_play_the_same() {
        let (song, labels) = groove_song();
        let (split, split_labels) = rewritten(&song, &labels, &split_plan(&labels, &song, 1).unwrap());
        assert_eq!(split.count(), 5);
        assert_eq!(split.repeats, [1, 1, 1, 1, 1]);
        assert_eq!(split.values[Arr::PARAM_LOOP], 2.0, "Loop still goes to the Groove's first copy");
        let names: Vec<_> = (0..5).map(|s| split_labels.get(&section_key(s)).cloned().unwrap_or_default()).collect();
        assert_eq!(names, ["Intro", "Groove", "Groove 2", "Groove 3", "Outro"]);
        assert_eq!(render(&split), render(&song));
        assert!(split_plan(&labels, &split, 1).is_none(), "nothing left to split");
    }

    #[test]
    fn merge_folds_the_copies_back_into_one() {
        let (song, labels) = groove_song();
        let (split, split_labels) = rewritten(&song, &labels, &split_plan(&labels, &song, 1).unwrap());
        assert_eq!(mergeable(&split_labels, &split, 1), 2);
        assert_eq!(mergeable(&split_labels, &split, 0), 0, "the Intro plays something else");
        let (merged, merged_labels) = rewritten(&split, &split_labels, &merge_plan(&split_labels, &split, 1, MAX_REPEATS).unwrap());
        assert_eq!(merged.count(), 3);
        assert_eq!(merged.repeats, song.repeats);
        assert_eq!(merged.values[..Arr::PARAM_LENGTH + 3], song.values[..Arr::PARAM_LENGTH + 3]);
        assert_eq!(merged_labels, labels, "the copies' names go with them");
        assert_eq!(render(&merged), render(&song));
    }

    #[test]
    fn merge_stops_where_the_music_or_the_loop_changes() {
        let (song, labels) = groove_song();
        let (mut split, split_labels) = rewritten(&song, &labels, &split_plan(&labels, &song, 1).unwrap());
        // The third copy hits harder: only the second folds in
        split.values[Arr::cue_param(3, 1)] = Cue::hit(900).encode();
        assert_eq!(mergeable(&split_labels, &Song::of(split.values.clone()), 1), 1);
        // Looping back to the second copy keeps it a section of its own
        let (mut split, split_labels) = rewritten(&song, &labels, &split_plan(&labels, &song, 1).unwrap());
        split.values[Arr::PARAM_LOOP] = 3.0;
        assert_eq!(mergeable(&split_labels, &Song::of(split.values), 1), 0);
    }
}
