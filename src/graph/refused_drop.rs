//! Saying why a cable won't go in (#496).
//!
//! The editor only offers a dragged cable the jacks that can take it, so one
//! let go over a jack that can't (Control onto a Gate, MIDI onto Audio, a
//! module into itself) simply vanished, with nothing to say why. Now the
//! jack says so, where the eye already is. While the cable hovers it, a ring
//! round the jack is broken in two: one half in the cable's colour, the
//! other in the jack's, not meeting. Let go, and the halves spring apart and
//! fade, while a small tag beside the jack gives the reason, its signal
//! names in their colours. The status bar says it too.
//!
//! Refusals the editor does let through to Soba (a loop, an Arranger
//! following something it can't) are shown the same way.

use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_2, PI};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, Vec2};
use egui_node_graph2::{drawn_port_rect, AnyParameterId, InputId, InputParamKind, NodeId, OutputId};

use super::{validate_connection, ConnectionError, SynthGraph, ValidationResult};
use crate::app::theme;
use crate::dsp::SignalType;

/// How near a jack the pointer must be to be over it, before zoom: the
/// editor's own reach for a jack that takes the cable.
const REACH: f32 = 10.0;
/// How long the broken ring takes to spring apart once let go, in seconds.
const SPRING: f32 = 0.45;
/// How long the reason stays beside the jack, its last [`FADE_OUT`] fading.
const LINGER: f32 = 2.8;
const FADE_IN: f32 = 0.08;
const FADE_OUT: f32 = 0.7;
/// The tag's words, and the widest it grows before wrapping.
const TAG_FONT: f32 = 12.5;
const TAG_WIDTH: f32 = 260.0;

/// A jack that won't take a cable: where, which signals, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub jack: AnyParameterId,
    /// The signal the cable carries, and the one the jack takes or gives
    pub cable: SignalType,
    pub jack_signal: SignalType,
    pub reason: String,
}

impl Refusal {
    /// A cable carrying `cable` held to `jack`, of `jack_signal`, on the
    /// module at the other end (`same_module` if it's the cable's own): the
    /// refusal if there is one. None if the jack takes it.
    pub fn of(jack: AnyParameterId, cable: SignalType, jack_signal: SignalType, same_module: bool) -> Option<Self> {
        // Always judged from output to input
        let (from, to) = match jack {
            AnyParameterId::Input(_) => (cable, jack_signal),
            AnyParameterId::Output(_) => (jack_signal, cable),
        };
        let reason = if same_module {
            ConnectionError::SelfConnection.message()
        } else {
            match validate_connection(from, to) {
                ValidationResult::Invalid(error) => error.message(),
                ValidationResult::Valid => return None,
            }
        };
        Some(Self { jack, cable, jack_signal, reason: reason.to_string() })
    }

    /// A cable from `output` into `input` that Soba turned away for `reason`
    /// once the editor had made it; `jack` is the end it was dropped on.
    pub fn of_cable(graph: &SynthGraph, output: OutputId, input: InputId, jack: AnyParameterId, reason: &str) -> Option<Self> {
        let signal = |param| graph.any_param_type(param).ok().map(|t| t.signal_type());
        let (from, to) = (signal(AnyParameterId::Output(output))?, signal(AnyParameterId::Input(input))?);
        let (cable, jack_signal) = match jack {
            AnyParameterId::Input(_) => (from, to),
            AnyParameterId::Output(_) => (to, from),
        };
        Some(Self { jack, cable, jack_signal, reason: reason.to_string() })
    }

    /// Whether the jack is an input, on a module's left edge: the cable comes
    /// to it from the left.
    fn comes_from_left(&self) -> bool {
        matches!(self.jack, AnyParameterId::Input(_))
    }
}

/// The jack under a dragged cable that won't take it, and the last one let
/// go on, as it fades.
#[derive(Default)]
pub struct RefusedDrop {
    hover: Option<Refusal>,
    shown: Option<(Refusal, f32)>,
}

impl RefusedDrop {
    /// Once the graph is drawn: notes the jack a cable dragged from
    /// `dragging` is over that won't take it. If the cable was let go there
    /// (`released`), it's shown as refused, and its reason returned for the
    /// status bar.
    pub fn follow(
        &mut self,
        ctx: &egui::Context,
        graph: &SynthGraph,
        dragging: Option<(NodeId, AnyParameterId)>,
        hidden: &HashSet<NodeId>,
        zoom: f32,
        released: bool,
    ) -> Option<String> {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        self.hover = dragging.zip(pointer).and_then(|(origin, pointer)| under(ctx, graph, origin, hidden, pointer, REACH * zoom));
        if !released {
            return None;
        }
        let refusal = self.hover.take()?;
        let reason = refusal.reason.clone();
        self.shown = Some((refusal, 0.0));
        Some(reason)
    }

    /// Shows `refusal` as just let go.
    pub fn refuse(&mut self, refusal: Refusal) {
        self.shown = Some((refusal, 0.0));
    }

    /// Paints the broken ring over the jack hovered, and the last refusal
    /// springing apart and its reason, `dt` seconds on.
    pub fn paint(&mut self, ctx: &egui::Context, zoom: f32, dt: f32) {
        if let Some((_, age)) = &mut self.shown {
            *age += dt;
            if *age > LINGER {
                self.shown = None;
            }
        }
        if self.hover.is_none() && self.shown.is_none() {
            return;
        }
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("refused_drop")));
        if let Some(refusal) = &self.hover {
            if let Some(rect) = drawn_port_rect(ctx, refusal.jack) {
                paint_ring(&painter, refusal, rect.center(), zoom, 0.0, true);
            }
        }
        if let Some((refusal, age)) = &self.shown {
            if let Some(rect) = drawn_port_rect(ctx, refusal.jack) {
                let spring = (age / SPRING).min(1.0);
                if spring < 1.0 {
                    paint_ring(&painter, refusal, rect.center(), zoom, spring, false);
                }
                paint_tag(&painter, refusal, rect.center(), zoom, tag_alpha(*age), ctx.content_rect());
            }
            ctx.request_repaint();
        }
    }
}

/// The jack within `reach` of `pointer` nearest it that a cable dragged from
/// `origin` can't go into, on a module shown at this level. None if the
/// nearest jack takes it, or none is that near.
fn under(
    ctx: &egui::Context,
    graph: &SynthGraph,
    origin: (NodeId, AnyParameterId),
    hidden: &HashSet<NodeId>,
    pointer: Pos2,
    reach: f32,
) -> Option<Refusal> {
    let (origin_node, origin_param) = origin;
    let cable = graph.any_param_type(origin_param).ok()?.signal_type();
    // The other kind of jack from the one the cable came out of
    let jacks: Vec<(NodeId, AnyParameterId)> = match origin_param {
        AnyParameterId::Output(_) => graph
            .inputs
            .iter()
            .filter(|(_, input)| !matches!(input.kind, InputParamKind::ConstantOnly))
            .map(|(id, input)| (input.node, AnyParameterId::Input(id)))
            .collect(),
        AnyParameterId::Input(_) => graph.outputs.iter().map(|(id, output)| (output.node, AnyParameterId::Output(id))).collect(),
    };
    let (node, jack, _) = jacks
        .into_iter()
        .filter(|(node, _)| !hidden.contains(node))
        .filter_map(|(node, jack)| Some((node, jack, drawn_port_rect(ctx, jack)?.center().distance(pointer))))
        .filter(|&(.., distance)| distance < reach)
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    let jack_signal = graph.any_param_type(jack).ok()?.signal_type();
    Refusal::of(jack, cable, jack_signal, node == origin_node)
}

/// How much of the reason shows, `age` seconds after the cable was let go.
fn tag_alpha(age: f32) -> f32 {
    let rise = (age / FADE_IN).min(1.0);
    let fall = ((LINGER - age) / FADE_OUT).clamp(0.0, 1.0);
    let fall = fall * fall * (3.0 - 2.0 * fall);
    rise * fall
}

/// The ring round a refused jack at `center`, broken into the cable's half
/// (on the side it comes from) and the jack's. `spring` from 0 to 1 pulls
/// the halves apart and fades them; `veil` first covers the jack, which the
/// editor lights as the pointer nears it, in its own colour.
fn paint_ring(painter: &egui::Painter, refusal: &Refusal, center: Pos2, zoom: f32, spring: f32, veil: bool) {
    let jack_radius = 5.0 * zoom;
    let radius = jack_radius + (2.5 * zoom).max(2.5);
    let width = (1.6 * zoom).clamp(1.2, 2.6);
    let eased = 1.0 - (1.0 - spring).powi(3);
    let fade = 1.0 - spring * spring;
    let jack_color = refusal.jack_signal.color();
    if veil {
        painter.circle_filled(center, jack_radius, theme::node::PORT_SHADOW);
        painter.circle_filled(center, jack_radius * 0.75, jack_color.gamma_multiply(0.45));
    }
    // The cable's half faces where it came from; the gaps open top and bottom
    let outward = if refusal.comes_from_left() { -1.0 } else { 1.0 };
    let gap = 0.42 + 0.35 * eased;
    let drift = outward * (1.0 + 5.0 * eased) * zoom.max(0.6);
    let halves = [
        (refusal.cable.color(), if refusal.comes_from_left() { PI } else { 0.0 }, drift),
        (jack_color, if refusal.comes_from_left() { 0.0 } else { PI }, -drift),
    ];
    for (color, facing, shift) in halves {
        let points = arc(center + Vec2::new(shift, 0.0), radius, facing, FRAC_PI_2 - gap);
        painter.add(Shape::line(points, Stroke::new(width, color.gamma_multiply(fade))));
    }
}

/// Points along an arc of `radius` round `center`, `reach` radians either side of `facing`.
fn arc(center: Pos2, radius: f32, facing: f32, reach: f32) -> Vec<Pos2> {
    const STEPS: usize = 14;
    (0..=STEPS)
        .map(|k| {
            let angle = facing - reach + 2.0 * reach * k as f32 / STEPS as f32;
            center + radius * Vec2::angled(angle)
        })
        .collect()
}

/// The reason, in a small dark tag beside the jack on the side the cable
/// came from, faded to `alpha` and kept on `screen`.
fn paint_tag(painter: &egui::Painter, refusal: &Refusal, jack: Pos2, zoom: f32, alpha: f32, screen: Rect) {
    if alpha <= 0.0 {
        return;
    }
    let galley = painter.layout_job(tag_text(refusal, alpha));
    let padding = Vec2::new(9.0, 5.0);
    let size = galley.size() + 2.0 * padding;
    // Just above the line cables come in on, leaving the jack's own clear
    let ring = 5.0 * zoom + (2.5 * zoom).max(2.5);
    let (clear, lift) = (ring + 6.0, ring.min(8.0) + 2.0);
    let (anchor, at) = if refusal.comes_from_left() {
        (Align2::RIGHT_BOTTOM, jack + Vec2::new(-clear, -lift))
    } else {
        (Align2::LEFT_BOTTOM, jack + Vec2::new(clear, -lift))
    };
    let rect = anchor.anchor_size(at, size);
    // Kept on screen: a jack at the edge moves it in
    let rect = rect.translate(Vec2::new(
        (screen.left() + 4.0 - rect.left()).max(0.0) - (rect.right() - screen.right() + 4.0).max(0.0),
        (screen.top() + 4.0 - rect.top()).max(0.0) - (rect.bottom() - screen.bottom() + 4.0).max(0.0),
    ));
    let rounding = size.y / 2.0;
    painter.rect_filled(rect.translate(Vec2::new(0.0, 2.0)), rounding, Color32::from_black_alpha((70.0 * alpha) as u8));
    painter.rect(
        rect,
        rounding,
        theme::background::PANEL.gamma_multiply(0.96 * alpha),
        Stroke::new(1.0, refusal.jack_signal.color().gamma_multiply(0.55 * alpha)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + padding, galley, theme::text::PRIMARY);
}

/// The reason's words, wrapped, its signal names in their colours.
fn tag_text(refusal: &Refusal, alpha: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = TAG_WIDTH;
    let font = FontId::proportional(TAG_FONT);
    let plain = TextFormat::simple(font.clone(), theme::text::PRIMARY.gamma_multiply(0.92 * alpha));
    let signals = [refusal.cable, refusal.jack_signal];
    for (word, signal) in tinted(&refusal.reason, &signals) {
        let format = match signal {
            Some(signal) => TextFormat::simple(font.clone(), signal.color().gamma_multiply(alpha)),
            None => plain.clone(),
        };
        job.append(word, 0.0, format);
    }
    job
}

/// `text` in pieces, each signal of `signals` named in it picked out.
fn tinted<'a>(text: &'a str, signals: &[SignalType]) -> Vec<(&'a str, Option<SignalType>)> {
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        let next = signals
            .iter()
            .filter_map(|&signal| rest.find(signal.name()).map(|at| (at, signal)))
            .min_by_key(|&(at, _)| at);
        let Some((at, signal)) = next else { break };
        let end = at + signal.name().len();
        if at > 0 {
            pieces.push((&rest[..at], None));
        }
        pieces.push((&rest[at..end], Some(signal)));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        pieces.push((rest, None));
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> AnyParameterId {
        AnyParameterId::Input(InputId::default())
    }

    fn output() -> AnyParameterId {
        AnyParameterId::Output(OutputId::default())
    }

    #[test]
    fn control_onto_a_gate_is_refused_with_its_reason() {
        let refusal = Refusal::of(input(), SignalType::Control, SignalType::Gate, false).unwrap();
        assert_eq!(refusal.reason, "Control cannot connect to Gate");
        assert!(refusal.comes_from_left());
    }

    #[test]
    fn a_jack_that_takes_the_cable_is_no_refusal() {
        assert!(Refusal::of(input(), SignalType::Gate, SignalType::Control, false).is_none());
        assert!(Refusal::of(input(), SignalType::Audio, SignalType::Control, false).is_none());
    }

    #[test]
    fn a_cable_dragged_back_from_an_input_is_judged_output_to_input() {
        // Dragged out of a Gate input onto a Control output: Control into Gate
        let refusal = Refusal::of(output(), SignalType::Gate, SignalType::Control, false).unwrap();
        assert_eq!(refusal.reason, "Control cannot connect to Gate");
        assert!(!refusal.comes_from_left());
        // ...and out of a Control input onto a Gate output is fine
        assert!(Refusal::of(output(), SignalType::Control, SignalType::Gate, false).is_none());
    }

    #[test]
    fn a_module_cant_patch_into_itself_even_where_the_signals_match() {
        let refusal = Refusal::of(input(), SignalType::Audio, SignalType::Audio, true).unwrap();
        assert_eq!(refusal.reason, ConnectionError::SelfConnection.message());
    }

    #[test]
    fn the_signals_named_in_a_reason_are_picked_out() {
        let pieces = tinted("Control cannot connect to Gate", &[SignalType::Control, SignalType::Gate]);
        assert_eq!(
            pieces,
            vec![
                ("Control", Some(SignalType::Control)),
                (" cannot connect to ", None),
                ("Gate", Some(SignalType::Gate)),
            ]
        );
        assert_eq!(tinted("A loop", &[SignalType::Audio]), vec![("A loop", None)]);
    }

    #[test]
    fn the_reason_comes_up_quickly_and_fades_by_the_end() {
        assert_eq!(tag_alpha(0.0), 0.0);
        assert_eq!(tag_alpha(1.0), 1.0);
        assert!(tag_alpha(LINGER - FADE_OUT / 2.0) < 1.0);
        assert_eq!(tag_alpha(LINGER), 0.0);
    }
}
