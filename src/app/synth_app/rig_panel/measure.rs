//! Measuring a port's device from the Rig (#449): ping a note, hear it
//! come back on the audio input, and offer the offset that lines it up.
//!
//! The ping glyph by a port's offset opens a line under its row: which
//! channel and note to ping, and Measure. While the pings play, the line
//! counts them; then it says how long the device took, how steady it was,
//! where it lands now, and offers to set the offset. The pings are drawn
//! one over another from the note, the part of the trip that's Soba's and
//! the interface's shaded, so a device that answers the same way each
//! time shows as one sharp edge.
//!
//! One measurement at a time, and none without an input open. Closing the
//! Rig stops one under way. See [`ping`](crate::link::ping) for how the
//! timing is worked out.

use super::*;
use crate::dsp::arrival::{agree, Agreement, Disagreement};
use crate::dsp::input_channels::{self, InputChannels};
use crate::link::ping::{Listening, Ping, PingPlan, Progress, Trip};

/// How long a measurement may hear nothing at all before it's given up:
/// the audio isn't running.
const NO_AUDIO: f64 = 2.0;
/// The most the input's jitter buffer may move while listening, in
/// seconds, before the timing can't be trusted.
const MOST_SHIFT: f64 = 0.0005;
/// An answer this many ms before the note left is impossible: something
/// else was heard, or a delay is misreported.
const TOO_EARLY: f32 = -1.0;
/// Closer than this to on time, in ms, is on time.
const ON_TIME: f32 = 0.5;
/// How many columns each ping's trace is drawn in.
const TRACE_COLUMNS: usize = 320;
/// How tall the traces are drawn.
const TRACE_HEIGHT: f32 = 34.0;

/// The Rig's measurement: which port's line is open, the note each port
/// pings, and the measurement under way or done.
pub(in crate::app::synth_app) struct Measure {
    /// The port whose line is open.
    port: Option<usize>,
    /// The channel (0 to 15) and note each port pings.
    notes: [(u8, u8); PORTS],
    /// The input's channels each port's device comes back on.
    listens: [InputChannels; PORTS],
    stage: Stage,
    /// Measurements stopped before the engine let go of them, kept until
    /// it does so their rings aren't freed on the audio thread.
    retiring: Vec<Listening>,
}

impl Default for Measure {
    fn default() -> Self {
        // Channel 1, middle C: a device's profile may know better
        Self { port: None, notes: [(0, 60); PORTS], listens: [InputChannels::default(); PORTS], stage: Stage::Ready, retiring: Vec::new() }
    }
}

impl std::fmt::Debug for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stage = match &self.stage {
            Stage::Ready => "ready",
            Stage::Listening { .. } => "listening",
            Stage::Done(Ok(_)) => "measured",
            Stage::Done(Err(_)) => "gave up",
        };
        f.debug_struct("Measure").field("port", &self.port).field("stage", &stage).finish()
    }
}

enum Stage {
    /// Waiting for Measure.
    Ready,
    Listening {
        listening: Listening,
        /// The input's counters once the pings begin, to tell whether its
        /// buffer moved while they played.
        before: Option<Counters>,
        /// The port's offset as the pings left.
        offset: f32,
        started: Instant,
        rate: u32,
    },
    /// What it found, or why it gave up.
    Done(Result<Found, String>),
}

/// What moves the input's timing: its jitter buffer's counters, in frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counters {
    underrun: u64,
    moved: u64,
}

impl Counters {
    fn of(monitor: &InputMonitor) -> Self {
        Self {
            underrun: monitor.underrun_frames(),
            moved: monitor.overflow_frames() + monitor.settled_frames() + monitor.stretched_frames(),
        }
    }
}

/// A measurement that worked.
struct Found {
    agreement: Agreement,
    /// The offset that lands it with Soba, and whether that's as far as
    /// the offset reaches rather than all the way.
    offset: f32,
    out_of_reach: bool,
    /// How late it lands now, and once the offset is set, in ms.
    lands: f32,
    lands_after: f32,
    /// What the trip besides the device took, in ms.
    besides: f32,
    /// Whether the input's delay was a guess (it reports none).
    input_estimated: bool,
    /// Whether every part of the trip was known.
    complete: bool,
    traces: Traces,
}

/// The pings as heard, one over another from the note.
struct Traces {
    /// How many ms after the note the traces run to.
    span: f32,
    /// Each ping's waveform across the span, each column's lowest and
    /// highest against its loudest, and where it was found to begin (ms
    /// after the note), if it was.
    pings: Vec<(Vec<(f32, f32)>, Option<f32>)>,
}

/// The audio input, as the line names it: its name, if one is open, and
/// how many channels it has.
pub(super) type Input<'a> = (Option<&'a str>, usize);

/// What the line asked for.
pub(in crate::app::synth_app) enum MeasureEdit {
    /// Opens a port's line, or closes it if it's open.
    Open(usize),
    Close,
    Start(usize),
    Stop,
    Note(usize, (u8, u8)),
    /// Which of the input's channels a port's device comes back on.
    Listen(usize, InputChannels),
    /// Sets a port's offset to what was measured.
    Set(usize, f32),
}

impl SynthApp {
    /// Keeps a measurement going: takes in what's been heard and, once
    /// every ping is in, works out what it found. While the Rig is closed,
    /// stops one under way.
    pub(super) fn tend_measurement(&mut self, rig_open: bool) {
        let measure = &mut self.rig_panel.measure;
        measure.retiring.retain(|listening| !listening.released());
        if !rig_open {
            if matches!(measure.stage, Stage::Listening { .. }) {
                self.stop_measuring();
            }
            return;
        }
        let Stage::Listening { listening, before, started, .. } = &mut measure.stage else { return };
        let progress = listening.poll();
        let plan = *listening.plan();
        // The pings begin once the quiet before the first does
        if before.is_none() && matches!(progress, Progress::Listening(heard) if heard >= plan.note_at(0) - plan.quiet) {
            *before = self.audio.input.as_ref().map(Counters::of);
        }
        match progress {
            Progress::Listening(0) if started.elapsed().as_secs_f64() > NO_AUDIO => {
                self.stop_measuring();
                self.rig_panel.measure.stage = Stage::Done(Err("Nothing came in: the audio isn't running. Start it, and measure again".into()));
            }
            Progress::Listening(_) => {}
            Progress::Cut(_) => {
                let Stage::Listening { listening, .. } = std::mem::replace(&mut self.rig_panel.measure.stage, Stage::Ready) else { return };
                self.rig_panel.measure.retiring.push(listening);
                self.rig_panel.measure.stage = Stage::Done(Err("Stopped before the pings were done: the input closed, or the audio stopped".into()));
            }
            Progress::Heard => {
                let Stage::Listening { listening, before, offset, rate, .. } = std::mem::replace(&mut self.rig_panel.measure.stage, Stage::Ready) else {
                    return;
                };
                let found = self.found(&listening, before, offset, rate);
                let measure = &mut self.rig_panel.measure;
                measure.retiring.push(listening);
                measure.stage = Stage::Done(found);
            }
        }
    }

    /// Applies what a measurement line asked for.
    pub(super) fn measure_edit(&mut self, edit: MeasureEdit) {
        let measure = &mut self.rig_panel.measure;
        match edit {
            MeasureEdit::Open(port) => {
                if measure.port == Some(port) && !matches!(measure.stage, Stage::Listening { .. }) {
                    measure.port = None;
                } else if !matches!(measure.stage, Stage::Listening { .. }) {
                    measure.port = Some(port);
                    measure.stage = Stage::Ready;
                }
            }
            MeasureEdit::Close => {
                self.stop_measuring();
                self.rig_panel.measure.port = None;
            }
            MeasureEdit::Start(port) => self.start_measuring(port),
            MeasureEdit::Stop => self.stop_measuring(),
            MeasureEdit::Note(port, note) => {
                if let Some(slot) = measure.notes.get_mut(port) {
                    *slot = note;
                }
            }
            MeasureEdit::Listen(port, listen) => {
                if let Some(slot) = measure.listens.get_mut(port) {
                    *slot = listen;
                }
            }
            MeasureEdit::Set(port, ms) => MIDI.set_offset(port, ms),
        }
    }

    /// Pings `port` and listens, or says why it can't.
    fn start_measuring(&mut self, port: usize) {
        if matches!(self.rig_panel.measure.stage, Stage::Listening { .. }) {
            return;
        }
        let refused = |why: &str| Stage::Done(Err(why.to_string()));
        let label = PORT_LABELS.get(port).copied().unwrap_or("?");
        let (channel, note) = self.rig_panel.measure.notes[port.min(PORTS - 1)];
        let listen = self.rig_panel.measure.listens[port.min(PORTS - 1)];
        let input_channels = self.audio.engine.as_ref().ok().and_then(|engine| engine.input_channels()).map_or(2, usize::from);
        self.rig_panel.measure.port = Some(port);
        let rate = self.audio.engine.as_ref().map(|engine| engine.sample_rate()).ok();
        let stage = if MIDI.chosen(port).is_none() {
            refused(&format!("Choose port {label}'s device first"))
        } else if MIDI.found()[port.min(PORTS - 1)].is_none() {
            refused(&format!("Port {label}'s device isn't plugged in"))
        } else if self.audio.input.as_ref().is_none_or(InputMonitor::failed) {
            refused("Open an audio input first (Input, in the toolbar), with the device's sound plugged into it")
        } else if !listen.fits(input_channels) {
            refused(&format!("The input has no channel {}: choose which of its {input_channels} the device comes back on", listen.label()))
        } else if let (Some(rate), Some(handle)) = (rate, self.ui_handle.as_mut()) {
            let lead = if self.is_playing { PingPlan::LEAD } else { PingPlan::LEAD_STOPPED };
            let plan = PingPlan::new(port, channel, note, rate as f32, lead).listening_on(listen);
            match Ping::on_bus(&MIDI, plan) {
                Some((ping, listening)) => {
                    handle.ping(Some(ping));
                    Stage::Listening { listening, before: None, offset: MIDI.offset(port), started: Instant::now(), rate }
                }
                None => refused("MIDI Out isn't running here"),
            }
        } else {
            refused("The audio isn't running")
        };
        self.rig_panel.measure.stage = stage;
    }

    /// Stops a measurement under way, keeping its end until the engine
    /// lets go of the other.
    fn stop_measuring(&mut self) {
        let measure = &mut self.rig_panel.measure;
        if let Stage::Listening { listening, .. } = std::mem::replace(&mut measure.stage, Stage::Ready) {
            measure.retiring.push(listening);
            if let Some(handle) = self.ui_handle.as_mut() {
                handle.ping(None);
            }
        }
    }

    /// What a measurement that heard every ping found, or why it can't be
    /// trusted.
    fn found(&self, listening: &Listening, before: Option<Counters>, offset: f32, rate: u32) -> Result<Found, String> {
        let monitor = self.audio.input.as_ref().ok_or("The input closed while listening")?;
        let engine = self.audio.engine.as_ref().map_err(|_| "The audio stopped while listening")?;
        if engine.sample_rate() != rate {
            return Err("The audio device changed while listening: measure again".into());
        }
        // The jitter buffer moving moves every ping after it
        let after = Counters::of(monitor);
        if let Some(before) = before {
            if after.underrun > before.underrun {
                return Err("The input ran dry while listening, which moves its timing: measure again".into());
            }
            let moved = after.moved.saturating_sub(before.moved) as f64 / rate.max(1) as f64;
            if moved > MOST_SHIFT {
                return Err(format!("The input was still settling (it moved {:.1} ms while listening): measure again, with the transport playing if it keeps happening", moved * 1000.0));
            }
        }

        let plan = listening.plan();
        let trip = Trip::new(RoundTrip::of(monitor, engine.output_latency(), rate), offset);
        let arrivals = plan.arrivals(listening.captured());
        let delays = trip.device_delays(&arrivals, rate as f32);
        let agreement = agree(&delays).map_err(|disagreement| match disagreement {
            Disagreement::TooFewHeard { heard, of } => format!(
                "Heard {heard} of {of} pings clear of the noise, too few to go on. Check the device plays ch {} {} and its sound reaches the input",
                plan.channel + 1,
                note_name(plan.note)
            ),
            Disagreement::Scattered { spread, heard, of } => format!(
                "The {heard} of {of} pings heard came back up to {spread:.1} ms apart, too unsteady to line up. A sound with a sharper attack may help"
            ),
        })?;
        if agreement.median < TOO_EARLY {
            return Err(format!(
                "It answered {:.1} ms before the note left: the input is hearing something else, or a delay is misreported",
                -agreement.median
            ));
        }
        let (offset_to_set, out_of_reach) = trip.offset_for(agreement.median);
        Ok(Found {
            agreement,
            offset: offset_to_set,
            out_of_reach,
            lands: trip.lands(agreement.median),
            lands_after: Trip::new(trip.round_trip, offset_to_set).lands(agreement.median),
            besides: trip.besides_device(),
            input_estimated: trip.round_trip.input_estimated,
            complete: trip.round_trip.complete(),
            traces: traces(listening.captured(), plan, &arrivals, rate as f32),
        })
    }
}

/// A note's name, as the pianos have it: 60 is C4.
fn note_name(note: u8) -> String {
    let names = crate::modules::quantizer::NOTE_NAMES;
    format!("{}{}", names[note as usize % 12], note as i32 / 12 - 1)
}

/// The pings' traces, from each note to a little past the latest found.
fn traces(capture: &[f32], plan: &PingPlan, arrivals: &[Option<usize>], rate: f32) -> Traces {
    let ms = |frames: usize| frames as f32 / rate * 1000.0;
    let latest = arrivals.iter().flatten().max().map_or(0.0, |&at| ms(at));
    let span = (latest * 1.25 + 8.0).clamp(20.0, ms(plan.window()).max(20.0));
    let frames = ((span / 1000.0 * rate) as usize).max(TRACE_COLUMNS);
    let per = frames.div_ceil(TRACE_COLUMNS);
    let pings = (0..plan.pings)
        .map(|k| {
            let at = plan.note_at(k);
            let heard = capture.get(at..(at + frames).min(capture.len())).unwrap_or_default();
            let loudest = heard.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-9);
            // Each column's lowest and highest, as a waveform is drawn
            let columns = heard
                .chunks(per)
                .map(|chunk| chunk.iter().fold((0.0f32, 0.0f32), |(low, high), &s| (low.min(s / loudest), high.max(s / loudest))))
                .collect();
            (columns, arrivals.get(k).copied().flatten().map(ms))
        })
        .collect();
    Traces { span, pings }
}

/// The ping glyph by a port's offset: opens its measurement line. It
/// sends rings out while the port is being measured.
pub(super) fn ping_button(ui: &mut egui::Ui, measure: &Measure, port: usize, edits: &mut Vec<Edit>) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, ROW_HEIGHT - 4.0), egui::Sense::click());
    let open = measure.port == Some(port);
    let listening = open && matches!(measure.stage, Stage::Listening { .. });
    let purple = theme::signal::MIDI;
    let ink = if response.hovered() || open { purple.lerp_to_gamma(theme::text::PRIMARY, 0.45) } else { theme::text::QUIET };
    let painter = ui.painter();
    let source = rect.left_center() + egui::vec2(3.5, 0.0);
    painter.circle_filled(source, 1.8, ink);
    let time = ui.input(|i| i.time) as f32;
    for ring in 0..2 {
        // Two arcs going out; while measuring they travel
        let travel = if listening { (time * 1.6 + ring as f32 * 0.5).fract() } else { 0.35 + 0.4 * ring as f32 };
        let radius = 3.0 + 8.0 * travel;
        let fade = if listening { 1.0 - travel } else { 1.0 - 0.35 * ring as f32 };
        let points: Vec<egui::Pos2> = (0..=8)
            .map(|i| {
                let angle = (-0.9 + 1.8 * i as f32 / 8.0) * 0.75;
                source + radius * egui::vec2(angle.cos(), angle.sin())
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.2, ink.gamma_multiply(fade))));
    }
    if listening {
        ui.ctx().request_repaint();
    }
    let label = PORT_LABELS.get(port).copied().unwrap_or("?");
    if response
        .on_hover_text(format!("Measure port {label}'s device: Soba pings it a few times, hears it on the audio input, and offers the offset that lines it up"))
        .clicked()
    {
        edits.push(Edit::Measure(MeasureEdit::Open(port)));
    }
}

/// The measurement line under `port`'s row, while it's open.
pub(super) fn measure_line(ui: &mut egui::Ui, measure: &Measure, port: usize, input: Input<'_>, edits: &mut Vec<Edit>) {
    if measure.port != Some(port) {
        return;
    }
    let label = PORT_LABELS.get(port).copied().unwrap_or("?");
    let purple = theme::signal::MIDI;
    let indent = COLUMNS[0] + COLUMNS[1] + 2.0 * GAP;
    egui::Frame::new()
        .fill(theme::background::PANEL.lerp_to_gamma(purple, 0.06))
        .stroke(egui::Stroke::new(1.0, purple.gamma_multiply(0.3)))
        .corner_radius(theme::ROUNDING_SMALL)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .outer_margin(egui::Margin { left: indent as i8, bottom: 4, ..Default::default() })
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 2.0);
            let send = |edits: &mut Vec<Edit>, edit| edits.push(Edit::Measure(edit));
            match &measure.stage {
                Stage::Ready => ready(ui, measure.notes[port.min(PORTS - 1)], measure.listens[port.min(PORTS - 1)], port, input, edits),
                Stage::Listening { listening, .. } => {
                    let plan = listening.plan();
                    let heard = listening.captured().len();
                    let pinged = (0..plan.pings).filter(|&k| heard > plan.note_at(k)).count();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Listening to port {label}")).color(theme::text::PRIMARY));
                        let says = if pinged == 0 { "letting the input settle".to_string() } else { format!("ping {pinged} of {}", plan.pings) };
                        ui.label(RichText::new(says).small().color(theme::text::QUIET));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Stop").clicked() {
                                send(edits, MeasureEdit::Stop);
                            }
                        });
                    });
                    progress(ui, heard as f32 / plan.frames().max(1) as f32, plan);
                }
                Stage::Done(Ok(found)) => result(ui, found, port, edits),
                Stage::Done(Err(why)) => {
                    ui.add(egui::Label::new(RichText::new(why).color(theme::accent::WARNING)).wrap());
                    ui.horizontal(|ui| {
                        if ui.small_button("Again").clicked() {
                            send(edits, MeasureEdit::Start(port));
                        }
                        if ui.small_button("Close").clicked() {
                            send(edits, MeasureEdit::Close);
                        }
                    });
                }
            }
        });
}

/// Which note to ping, which of the input's channels to hear it on, and
/// the way to start.
fn ready(ui: &mut egui::Ui, (channel, note): (u8, u8), listen: InputChannels, port: usize, input: Input<'_>, edits: &mut Vec<Edit>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ping").color(theme::text::SECONDARY));
        let mut ch = channel as u32 + 1;
        let mut key = note as u32;
        let ch_drag = ui.add(egui::DragValue::new(&mut ch).range(1..=16).speed(0.1).prefix("ch "));
        let key_drag = ui.add(egui::DragValue::new(&mut key).range(0..=127).speed(0.2).custom_formatter(|n, _| format!("{} ({n})", note_name(n as u8))));
        ch_drag.on_hover_text("The channel the device plays: a drum machine's track, a synth's channel");
        key_drag.on_hover_text("A note with a sharp attack measures best: a kick, a rim, a pluck");
        if (ch, key) != (channel as u32 + 1, note as u32) {
            edits.push(Edit::Measure(MeasureEdit::Note(port, ((ch - 1) as u8, key as u8))));
        }
        ui.label(RichText::new(format!("{} times, and listen on", PingPlan::PINGS)).color(theme::text::SECONDARY));
        let (name, width) = input;
        let shown = if listen.fits(width) { theme::text::PRIMARY } else { theme::accent::WARNING };
        let response = egui::ComboBox::from_id_salt(("rig_measure_listen", port))
            .width(64.0)
            .selected_text(RichText::new(format!("In {}", listen.label())).color(shown))
            .show_ui(ui, |ui| {
                // Only the channels the input has: a pair, then each of its own
                for choice in (0..input_channels::LABELS.len()).map(InputChannels::from_index).filter(|choice| choice.fits(width)) {
                    if ui.selectable_label(choice == listen, choice.label()).clicked() && choice != listen {
                        edits.push(Edit::Measure(MeasureEdit::Listen(port, choice)));
                    }
                }
            })
            .response;
        let through = name.unwrap_or("the audio input");
        response.on_hover_text(format!("Which of {through}'s channels the device comes back on: a pair is heard as one"));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if cross(ui, 12.0).on_hover_text("Close").clicked() {
                edits.push(Edit::Measure(MeasureEdit::Close));
            }
            let start = ui.button(RichText::new("Measure").color(theme::text::PRIMARY));
            if start.on_hover_text("Plays the note and times each answer: quieten the patch's own notes on this port first").clicked() {
                edits.push(Edit::Measure(MeasureEdit::Start(port)));
            }
        });
    });
}

/// A thin bar across the measurement, a tick at each ping.
fn progress(ui: &mut egui::Ui, done: f32, plan: &PingPlan) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, theme::background::WIDGET);
    let mut filled = rect;
    filled.set_width(rect.width() * done.clamp(0.0, 1.0));
    painter.rect_filled(filled, 3.0, theme::signal::MIDI.gamma_multiply(0.7));
    for k in 0..plan.pings {
        let x = rect.left() + rect.width() * plan.note_at(k) as f32 / plan.frames().max(1) as f32;
        let lit = (x - rect.left()) <= filled.width();
        let ink = if lit { theme::text::PRIMARY } else { theme::text::DISABLED };
        painter.circle_filled(egui::pos2(x, rect.center().y), 2.0, ink);
    }
}

/// What a measurement found: the delay, the pings drawn, and the offset.
fn result(ui: &mut egui::Ui, found: &Found, port: usize, edits: &mut Vec<Edit>) {
    let Agreement { median, spread, heard, kept, of } = found.agreement;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Answers {median:.1} ms after the note")).color(theme::text::PRIMARY).strong());
        let left_out = if kept < heard { format!(", {} far off left out", heard - kept) } else { String::new() };
        ui.label(RichText::new(format!("within {spread:.1} ms over {kept} of {of} pings{left_out}")).small().color(theme::text::QUIET));
    });
    traces_strip(ui, found, median);
    ui.horizontal_wrapped(|ui| {
        if found.lands.abs() < ON_TIME {
            ui.label(RichText::new("Lands with Soba now").color(theme::signal::GATE));
        } else {
            let (how, by) = if found.lands > 0.0 { ("late", found.lands) } else { ("early", -found.lands) };
            ui.label(RichText::new(format!("Lands {by:.1} ms {how} now")).color(theme::text::SECONDARY));
            let set = ui.button(RichText::new(format!("Set Offset to {:+.1} ms", found.offset)).color(theme::text::PRIMARY));
            let tip = if found.out_of_reach {
                format!(
                    "As early as the port goes: the output's delay is all the head start there is, so it still lands {:.1} ms late. A larger buffer gives more",
                    found.lands_after
                )
            } else {
                "Moves its notes, CCs and clock by this much, so its sound lands with Soba's".to_string()
            };
            if set.on_hover_text(tip).clicked() {
                edits.push(Edit::Measure(MeasureEdit::Set(port, found.offset)));
            }
        }
        if ui.small_button("Again").clicked() {
            edits.push(Edit::Measure(MeasureEdit::Start(port)));
        }
        if ui.small_button("Close").clicked() {
            edits.push(Edit::Measure(MeasureEdit::Close));
        }
    });
    let mut caveats = Vec::new();
    if found.out_of_reach {
        caveats.push("The offset can't come early enough to catch it all up: a larger audio buffer gives more room".to_string());
    }
    if found.input_estimated {
        caveats.push("The input doesn't report its own delay, so it's taken as one packet, the least it can be: the device may be a little quicker than this".to_string());
    }
    if !found.complete {
        caveats.push("The audio device doesn't report all its delays, so only the parts it does are taken away".to_string());
    }
    for caveat in caveats {
        ui.add(egui::Label::new(RichText::new(caveat).small().color(theme::text::QUIET)).wrap());
    }
}

/// The pings drawn one over another from the note: Soba's and the
/// interface's part of the trip shaded, a tick where each was found to
/// begin, and a line where the device's sound does.
fn traces_strip(ui: &mut egui::Ui, found: &Found, median: f32) {
    let traces = &found.traces;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), TRACE_HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::background::MAIN);
    let x = |ms: f32| rect.left() + rect.width() * (ms / traces.span).clamp(0.0, 1.0);
    let answer = x(found.besides + median);
    // Soba's part and the interface's, then the device's
    let ours = egui::Rect::from_min_max(rect.left_top(), egui::pos2(x(found.besides), rect.bottom()));
    painter.rect_filled(ours, 3.0, theme::background::GRID);
    let theirs = egui::Rect::from_min_max(egui::pos2(ours.right(), rect.top()), egui::pos2(answer, rect.bottom()));
    painter.rect_filled(theirs, 0.0, theme::signal::MIDI.gamma_multiply(0.14));

    // Every ping's waveform, faint, so where they agree builds up bright
    let column = rect.width() / TRACE_COLUMNS as f32;
    let middle = rect.center().y;
    let reach = TRACE_HEIGHT * 0.5 - 3.0;
    let ink = theme::signal::AUDIO.gamma_multiply(0.32);
    for (columns, _) in &traces.pings {
        for (i, &(low, high)) in columns.iter().enumerate() {
            let at = rect.left() + (i as f32 + 0.5) * column;
            let (top, bottom) = (middle - high * reach, middle - low * reach);
            painter.line_segment([egui::pos2(at, top.min(middle - 0.5)), egui::pos2(at, bottom.max(middle + 0.5))], egui::Stroke::new(column.max(1.0), ink));
        }
    }
    for (_, at) in &traces.pings {
        if let Some(at) = at {
            let x = x(*at);
            painter.line_segment([egui::pos2(x, rect.top() + 2.0), egui::pos2(x, rect.top() + 6.0)], egui::Stroke::new(1.0, theme::text::SECONDARY));
        }
    }
    painter.line_segment([egui::pos2(answer, rect.top()), egui::pos2(answer, rect.bottom())], egui::Stroke::new(1.5, theme::signal::GATE.gamma_multiply(0.85)));

    // What each part is, where there's room to say
    let small = egui::FontId::proportional(10.0);
    let say = |text: String, left: f32, right: f32, color: egui::Color32| {
        let galley = painter.layout_no_wrap(text, small.clone(), color);
        if galley.size().x + 8.0 < right - left {
            painter.galley(egui::pos2(left + 4.0, rect.bottom() - 3.0 - galley.size().y), galley, color);
        }
    };
    say(format!("Soba and the interface, {:.1} ms", found.besides), ours.left(), ours.right(), theme::text::QUIET);
    say(format!("device, {median:.1} ms"), theirs.left(), theirs.right(), theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.5));
    let end = format!("{:.0} ms", traces.span);
    let galley = painter.layout_no_wrap(end, small, theme::text::DISABLED);
    painter.galley(egui::pos2(rect.right() - 4.0 - galley.size().x, rect.bottom() - 3.0 - galley.size().y), galley, theme::text::DISABLED);
    response.on_hover_text(format!(
        "Each ping as heard, from its note: where they agree, they build up bright. Shaded grey, the {:.1} ms that's Soba's and the interface's (the output's delay and the port's offset before the note leaves, the input's delay and buffer once the sound is back). Purple, the device's own {median:.1} ms. Each tick is where one ping was found to begin",
        found.besides
    ));
}
