//! Where a capture's pointer cues aim (#239): a point in the window, or a
//! part of a module, found where it was last drawn. A script aimed at parts
//! survives what moves them: another `--ppp`, a row added above, a frame
//! moved, a module grown.
//!
//! ```text
//! move 400 300 0.6                     # a point in the window
//! move by 0 -60 2.0                    # from where the pointer is
//! move source.sampler file 0.3         # the middle of a face's button
//! move osc.sine#2 knob:Detune 0.3      # a knob's dial
//! move vca in:CV 0.3                   # a jack: in:, out:, or port: for either
//! move fx.eq 234 135 0.3               # patch points from the module's corner
//! ```
//!
//! A module is named as `param` names it: its registry id, `#n` for the nth
//! of its kind. Knobs are `knob:<parameter>`, and a module with a bypass
//! switch has `bypass`; the other names a face marks are listed in
//! [`marks::FACE`]. A name the module can't have fails as the script loads.

use eframe::egui::{self, Pos2, Vec2};
use egui_node_graph2::{drawn_port_rect, AnyParameterId, NodeId};

use crate::dsp::bypass::can_bypass;
use crate::graph::{annotation_ui, catalog, marks, SynthGraph};

/// Where a pointer cue goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Spot {
    /// A point in the window, in points.
    Point(Pos2),
    /// This far from where the pointer is (or is gliding to).
    By(Vec2),
    /// A part of a module.
    Part(Part),
}

/// A part of the `nth` module of a kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub module: String,
    pub nth: usize,
    pub at: PartAt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartAt {
    /// A knob, jack or face button, by name.
    Named(String),
    /// Patch points from the module's top-left corner.
    Offset(Vec2),
}

impl std::fmt::Display for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} #{}", self.module, self.nth + 1)?;
        match &self.at {
            PartAt::Named(name) => write!(f, " {name}"),
            PartAt::Offset(v) => write!(f, " ({}, {})", v.x, v.y),
        }
    }
}

/// A module named as `kind` or `kind#n`, counted from 1, as (kind, index).
pub fn module_ref(target: &str) -> Result<(String, usize), &'static str> {
    match target.split_once('#') {
        Some((m, n)) => Ok((m.to_string(), n.parse::<usize>().map_err(|_| "bad #n")?.max(1) - 1)),
        None => Ok((target.to_string(), 0)),
    }
}

/// Reads a spot from a cue's words after the cue's name, and the numbers
/// after it (a glide's length, say).
pub fn parse_spot(words: &[&str]) -> Result<(Spot, Vec<f64>), &'static str> {
    let number = |w: &&str| w.parse::<f64>().ok();
    let numbers = |ws: &[&str]| ws.iter().map(|w| number(w).ok_or("not a number")).collect::<Result<Vec<_>, _>>();
    let first = *words.first().ok_or("missing point or module")?;
    if number(&first).is_some() {
        let n = numbers(words)?;
        let [x, y, rest @ ..] = n.as_slice() else { return Err("missing value") };
        return Ok((Spot::Point(Pos2::new(*x as f32, *y as f32)), rest.to_vec()));
    }
    if first == "by" {
        let n = numbers(&words[1..])?;
        let [x, y, rest @ ..] = n.as_slice() else { return Err("missing value") };
        return Ok((Spot::By(Vec2::new(*x as f32, *y as f32)), rest.to_vec()));
    }
    let (module, nth) = module_ref(first)?;
    // A part's name may have spaces, as an input's does: the words up to
    // the numbers
    let rest = &words[1..];
    let name_len = rest.iter().take_while(|w| number(w).is_none()).count();
    let name = rest[..name_len].join(" ");
    let n = numbers(&rest[name_len..])?;
    let (at, rest) = if name.is_empty() {
        let [x, y, rest @ ..] = n.as_slice() else { return Err("expected a part, or x y on the module") };
        (PartAt::Offset(Vec2::new(*x as f32, *y as f32)), rest.to_vec())
    } else {
        (PartAt::Named(name), n)
    };
    Ok((Spot::Part(Part { module, nth, at }), rest))
}

/// Checks a part against what a module of its kind has, so a misspelt
/// script fails as it loads instead of partway through a long film.
pub fn check(part: &Part) -> Result<(), String> {
    let spec = catalog::module(&part.module).ok_or_else(|| format!("no module kind {}", part.module))?;
    let PartAt::Named(name) = &part.at else { return Ok(()) };
    let is = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let found = match name.split_once(':') {
        Some(("knob", knob)) => spec.parameters.iter().any(|p| is(p.name, knob)),
        // An input with a jack is named for its port or the parameter it carries
        Some(("in", jack)) => spec.inputs().any(|p| is(p.name, jack)) || spec.parameters.iter().any(|p| is(p.name, jack)),
        Some(("out", jack)) => spec.outputs().any(|p| is(p.name, jack)),
        Some(("port", jack)) => spec.ports.iter().any(|p| is(p.name, jack)) || spec.parameters.iter().any(|p| is(p.name, jack)),
        _ => {
            (is(name, "bypass") && can_bypass(spec.info.category, &spec.ports))
                || marks::face_names(&part.module).iter().any(|n| is(n, name))
        }
    };
    if found {
        return Ok(());
    }
    let mut known: Vec<String> = marks::face_names(&part.module).iter().map(|n| n.to_string()).collect();
    if can_bypass(spec.info.category, &spec.ports) {
        known.push("bypass".into());
    }
    known.extend(spec.parameters.iter().map(|p| format!("knob:{}", p.name)));
    known.extend(spec.inputs().map(|p| format!("in:{}", p.name)));
    known.extend(spec.outputs().map(|p| format!("out:{}", p.name)));
    Err(format!("{} has no {name}; it has {}", part.module, known.join(", ")))
}

/// Where a part is on screen now: the middle of where it was last drawn.
pub fn locate(ctx: &egui::Context, graph: &SynthGraph, zoom: f32, part: &Part) -> Result<Pos2, String> {
    let node_id = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.user_data.module_id == part.module)
        .nth(part.nth)
        .map(|(id, _)| id)
        .ok_or_else(|| format!("finds no {} #{} in the patch", part.module, part.nth + 1))?;
    let module = annotation_ui::module_rect(ctx, node_id)
        .ok_or_else(|| format!("finds {} #{} isn't on show", part.module, part.nth + 1))?;
    let name = match &part.at {
        PartAt::Offset(v) => return Ok(module.min + *v * zoom),
        PartAt::Named(name) => name,
    };
    let rect = match jack(graph, node_id, name) {
        Some(param) => drawn_port_rect(ctx, param),
        None => marks::marked(ctx, node_id, name),
    };
    rect.map(|r| r.center()).ok_or_else(|| {
        format!("finds no {name} drawn on {} #{}: it shows {}", part.module, part.nth + 1, marks::names(ctx, node_id).join(", "))
    })
}

/// The jack an `in:`, `out:` or `port:` name means on a module, if it is one.
fn jack(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<AnyParameterId> {
    let (kind, jack) = name.split_once(':')?;
    let node = &graph.nodes[node_id];
    let input = || node.inputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(jack)).map(|(_, id)| AnyParameterId::Input(*id));
    let output = || node.outputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(jack)).map(|(_, id)| AnyParameterId::Output(*id));
    match kind {
        "in" => input(),
        "out" => output(),
        "port" => input().or_else(output),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(line: &str) -> Result<(Spot, Vec<f64>), &'static str> {
        parse_spot(&line.split_whitespace().collect::<Vec<_>>())
    }

    fn part(module: &str, nth: usize, at: PartAt) -> Spot {
        Spot::Part(Part { module: module.into(), nth, at })
    }

    #[test]
    fn a_spot_is_a_point_a_step_or_a_part() {
        assert_eq!(spot("400 300 0.6"), Ok((Spot::Point(Pos2::new(400.0, 300.0)), vec![0.6])));
        assert_eq!(spot("by 0 -60"), Ok((Spot::By(Vec2::new(0.0, -60.0)), vec![])));
        let named = |name: &str| PartAt::Named(name.into());
        assert_eq!(spot("source.sampler file 0.3"), Ok((part("source.sampler", 0, named("file")), vec![0.3])));
        assert_eq!(spot("osc.sine#2 knob:Detune"), Ok((part("osc.sine", 1, named("knob:Detune")), vec![])));
        // Names with spaces, up to the numbers
        assert_eq!(spot("util.clock knob:Gate Length 1"), Ok((part("util.clock", 0, named("knob:Gate Length")), vec![1.0])));
        assert_eq!(spot("fx.eq 234 135.5 0.3"), Ok((part("fx.eq", 0, PartAt::Offset(Vec2::new(234.0, 135.5))), vec![0.3])));
        assert!(spot("fx.eq 0.3").is_err(), "a module needs a part or a point on it");
        assert!(spot("fx.eq knob:Gain 0.3 high").is_err());
        assert!(spot("400").is_err());
        assert!(spot("by 3").is_err());
    }

    #[test]
    fn a_part_the_module_cant_have_fails() {
        let named = |module: &str, name: &str| Part { module: module.into(), nth: 0, at: PartAt::Named(name.into()) };
        assert!(check(&named("source.sampler", "file")).is_ok());
        assert!(check(&named("source.sampler", "FILE")).is_ok(), "names match without case");
        assert!(check(&named("osc.sine", "knob:Detune")).is_ok());
        assert!(check(&named("filter.svf", "in:Cutoff")).is_ok(), "an input named for its parameter");
        assert!(check(&named("fx.delay", "bypass")).is_ok());
        let err = check(&named("source.sampler", "fil")).unwrap_err();
        assert!(err.contains("file") && err.contains("knob:"), "{err}");
        assert!(check(&named("osc.sine", "bypass")).is_err(), "an oscillator has no bypass switch");
        assert!(check(&named("osc.sine", "out:Nope")).is_err());
        assert!(check(&named("osc.nope", "knob:Detune")).is_err());
        assert!(check(&Part { module: "fx.eq".into(), nth: 0, at: PartAt::Offset(Vec2::ZERO) }).is_ok());
    }
}
