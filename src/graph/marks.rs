//! Named parts of a module, for a capture script to aim the pointer at
//! (#239). `move source.sampler file 0.3` lands on the Sampler's file button
//! wherever the face has put it: at any `--ppp`, after a row is added above
//! it, in whichever frame the module has moved to.
//!
//! Knobs mark themselves as `knob:<parameter>`, as do the toggles and
//! dropdowns an input draws beside its name. A face marks the parts a script
//! might press, under the names [`FACE`] lists, and a module with a bypass
//! switch marks it as `bypass`. Ports aren't marked here: they're found
//! where egui_node_graph2 last drew their jacks.
//!
//! Marking costs nothing until a capture asks for it with [`start`].

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Id, Rect};
use egui_node_graph2::NodeId;

static MARKING: AtomicBool = AtomicBool::new(false);

/// The names faces mark, by module: what a script can aim at beyond knobs,
/// ports and the bypass switch. A part in `<angle brackets>` is a number
/// counted from 1, so `step:<n>` is `step:1`, `step:2` and on, as far as the
/// pattern goes. Steps and cells are the ones on show: the pattern being
/// edited, the sections in the Arranger's view.
pub const FACE: &[(&str, &[&str])] = &[
    ("source.sampler", &["file", "cue", "rec"]),
    ("source.chopper", &["rec"]),
    ("seq.step", &["pattern:A", "pattern:B", "pattern:C", "pattern:D", "step:<n>"]),
    ("seq.trigger", &["pattern:A", "pattern:B", "pattern:C", "pattern:D", "cell:<lane>:<step>", "accent:<step>"]),
    ("seq.chord", &["step:<n>"]),
    ("util.looper", &["pedal:Rec", "pedal:Stop", "pedal:Undo", "pedal:Clear"]),
    ("seq.arranger", &["section:<n>", "cell:<lane>:<section>"]),
];

/// The names a module of this kind's face marks.
pub fn face_names(module_id: &str) -> &'static [&'static str] {
    FACE.iter().find(|(id, _)| *id == module_id).map_or(&[], |(_, names)| names)
}

/// Whether a module of this kind's face marks a name: `step:12` is one of
/// `step:<n>`. Without regard to case, as [`marked`] finds it.
pub fn is_face_name(module_id: &str, name: &str) -> bool {
    face_names(module_id).iter().any(|pattern| {
        let (mut want, mut got) = (pattern.split(':'), name.split(':'));
        loop {
            match (want.next(), got.next()) {
                (None, None) => return true,
                (Some(w), Some(g)) if w.starts_with('<') && g.parse::<usize>().is_ok_and(|n| n >= 1) => {}
                (Some(w), Some(g)) if !w.starts_with('<') && w.eq_ignore_ascii_case(g) => {}
                _ => return false,
            }
        }
    })
}

/// From now on, parts are marked as they're drawn.
pub fn start() {
    MARKING.store(true, Ordering::Relaxed);
}

/// What one module marked, in the last pass that drew it.
#[derive(Clone, Default)]
struct Marks {
    pass: u64,
    rects: Vec<(String, Rect)>,
}

fn marks_id(node_id: NodeId) -> Id {
    Id::new((node_id, "marks"))
}

/// Marks a part of a module as drawn at `rect`, on screen, this pass.
/// Call it every pass the part is drawn.
pub fn mark(ctx: &egui::Context, node_id: NodeId, name: impl std::fmt::Display, rect: Rect) {
    if !MARKING.load(Ordering::Relaxed) {
        return;
    }
    let name = name.to_string();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        let marks = data.get_temp_mut_or_default::<Marks>(marks_id(node_id));
        // A part the module stopped drawing goes with the pass before
        if marks.pass != pass {
            marks.pass = pass;
            marks.rects.clear();
        }
        marks.rects.retain(|(n, _)| *n != name);
        marks.rects.push((name, rect));
    });
}

/// Where a module's part was last drawn, on screen. Names match without
/// regard to case, as a capture's `param` inputs do.
pub fn marked(ctx: &egui::Context, node_id: NodeId, name: &str) -> Option<Rect> {
    ctx.data(|data| {
        let marks = data.get_temp::<Marks>(marks_id(node_id))?;
        marks.rects.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, r)| *r)
    })
}

/// The names a module marked when it was last drawn, for an error that
/// says what there is.
pub fn names(ctx: &egui::Context, node_id: NodeId) -> Vec<String> {
    ctx.data(|data| data.get_temp::<Marks>(marks_id(node_id)).map(|m| m.rects.into_iter().map(|(n, _)| n).collect()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_is_found_where_it_was_last_drawn() {
        let ctx = egui::Context::default();
        let node = NodeId::default();
        let rect = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(30.0, 12.0));
        start();
        mark(&ctx, node, "file", rect);
        mark(&ctx, node, "knob:Detune", rect.translate(egui::vec2(0.0, 40.0)));
        assert_eq!(marked(&ctx, node, "FILE"), Some(rect));
        assert_eq!(marked(&ctx, node, "knob:detune").map(|r| r.top()), Some(60.0));
        assert_eq!(marked(&ctx, node, "cue"), None);
        assert_eq!(names(&ctx, node), ["file", "knob:Detune"]);
        // Every name a face marks belongs to a module that's registered
        for (id, _) in FACE {
            assert!(super::super::catalog::module(id).is_some(), "{id}");
        }
        assert_eq!(face_names("source.sampler"), ["file", "cue", "rec"]);
        assert!(face_names("osc.sine").is_empty());
    }

    #[test]
    fn a_numbered_part_is_one_of_its_pattern() {
        assert!(is_face_name("seq.chord", "step:2"));
        assert!(is_face_name("seq.chord", "STEP:16"));
        assert!(is_face_name("seq.trigger", "cell:3:9"));
        assert!(is_face_name("util.looper", "pedal:rec"));
        assert!(is_face_name("source.sampler", "File"));
        assert!(!is_face_name("seq.chord", "step:0"), "steps count from 1");
        assert!(!is_face_name("seq.chord", "step:two"));
        assert!(!is_face_name("seq.chord", "step:<n>"));
        assert!(!is_face_name("seq.chord", "step"));
        assert!(!is_face_name("seq.trigger", "cell:3"));
        assert!(!is_face_name("seq.trigger", "cell:3:9:1"));
        assert!(!is_face_name("osc.sine", "step:1"));
    }
}
