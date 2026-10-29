//! Edits made from the keyboard and menus: adding, deleting, resetting,
//! copying, pasting and duplicating modules, and the frames and notes
//! selected with them.
//!
//! Each edit changes the editor graph and returns the engine commands that
//! make the same change to the audio graph. None of them records an undo
//! step: [`super::undo::History`] notices whatever changed once the frame is
//! over, the same way it notices edits made with the mouse.
//!
//! Positions here are editor node positions, which are in zoomed points:
//! a node is drawn at `position + pan + editor_rect.min`. Frames and notes
//! are kept in patch space instead (see [`crate::graph::annotations`]).

use std::collections::HashSet;

use egui::{Pos2, Vec2};
use egui_node_graph2::{NodeId, NodeTemplateTrait};

use crate::engine::EngineCommand;
use crate::graph::annotations::{Annotation, AnnotationId};
use crate::graph::groups::{self, GroupIndex};
use crate::graph::{port_mapping, NodeKind, SynthGraphEditorState, SynthGraphState, SynthNodeTemplate};
use crate::persistence::{capture_level, merge_patch, Patch, PatchError};
use super::engine_sync;

/// How far a duplicate lands from its original, in unzoomed points.
pub const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// The name copied modules travel under on the clipboard.
const CLIPBOARD_NAME: &str = "Copied modules";

/// Modules, and frames and notes, that an edit acts on together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub nodes: Vec<NodeId>,
    pub annotations: Vec<AnnotationId>,
}

impl Selection {
    /// Just these modules.
    pub fn modules(nodes: &[NodeId]) -> Self {
        Self { nodes: nodes.to_vec(), annotations: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.annotations.is_empty()
    }
}

/// A node's editor position in patch space.
fn to_patch(editor: &SynthGraphEditorState, user_state: &SynthGraphState, position: Pos2) -> Pos2 {
    ((position.to_vec2() - user_state.view_origin) / editor.pan_zoom.zoom).to_pos2()
}

/// The editor position of a point in patch space.
fn from_patch(editor: &SynthGraphEditorState, user_state: &SynthGraphState, position: Pos2) -> Pos2 {
    (user_state.view_origin + position.to_vec2() * editor.pan_zoom.zoom).to_pos2()
}

/// The top-left corner of a selection, in patch space.
fn patch_top_left(editor: &SynthGraphEditorState, user_state: &SynthGraphState, selection: &Selection) -> Option<Pos2> {
    let nodes = top_left(editor, &selection.nodes).map(|p| to_patch(editor, user_state, p));
    let annotations = user_state.annotations.top_left(&selection.annotations);
    nodes.into_iter().chain(annotations).reduce(|a, b| a.min(b))
}

/// Builds a node from a template and puts it at `position`, on top of the
/// others, on the level the editor shows. It has no engine ID yet.
pub fn place_node(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    template: SynthNodeTemplate,
    position: Pos2,
) -> NodeId {
    let node_id = editor.graph.add_node(
        template.node_graph_label(user_state),
        template.user_data(user_state),
        |graph, node_id| template.build_node(graph, user_state, node_id),
    );
    // On the level the editor shows
    editor.graph[node_id].user_data.parent = user_state.level;
    editor.node_positions.insert(node_id, position);
    editor.node_order.push(node_id);
    node_id
}

/// Adds a new module at `position`.
pub fn add_module(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    template: SynthNodeTemplate,
    position: Pos2,
) -> (NodeId, Vec<EngineCommand>) {
    let node_id = place_node(editor, user_state, template, position);
    // A Pad beside others takes the next note (#481)
    crate::graph::pads_follow_on(&mut editor.graph, &[node_id]);
    let engine_node_id = user_state.allocate_engine_node_id(node_id);
    (node_id, engine_sync::add_module(&editor.graph, node_id, engine_node_id))
}

/// Removes a node and its cables, and its module if it's a module. The
/// engine's cables aren't brought in line: see [`delete_modules`].
pub fn remove_module(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    node_id: NodeId,
) -> Vec<EngineCommand> {
    if !editor.graph.nodes.contains_key(node_id) {
        return Vec::new();
    }
    editor.graph.remove_node(node_id);
    editor.node_positions.remove(node_id);
    editor.node_order.retain(|id| *id != node_id);
    editor.selected_nodes.retain(|id| *id != node_id);
    user_state.remove_node(node_id)
        .map(|engine_node_id| EngineCommand::RemoveModule { node_id: engine_node_id })
        .into_iter()
        .collect()
}

/// The nodes deleting these takes away: each group goes with everything in
/// it. A group's Inputs and Outputs only go with the group.
pub fn deleted_with(graph: &crate::graph::SynthGraph, nodes: &[NodeId]) -> Vec<NodeId> {
    let index = GroupIndex::of(graph);
    let mut doomed = Vec::new();
    for &node_id in nodes {
        let Some(node) = graph.nodes.get(node_id) else { continue };
        match node.user_data.kind {
            NodeKind::Inputs(_) | NodeKind::Outputs(_) => continue,
            NodeKind::Group(id) => doomed.extend(groups::descendants(graph, &index, id)),
            NodeKind::Module => {}
        }
        doomed.push(node_id);
    }
    let mut seen = HashSet::new();
    doomed.retain(|n| seen.insert(*n));
    doomed
}

/// Removes several nodes, with their cables and modules, and groups with
/// everything inside them.
pub fn delete_modules(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    nodes: &[NodeId],
) -> Vec<EngineCommand> {
    let mut commands: Vec<EngineCommand> = deleted_with(&editor.graph, nodes)
        .into_iter()
        .flat_map(|node_id| remove_module(editor, user_state, node_id))
        .collect();
    commands.extend(engine_sync::sync_cables(&editor.graph, user_state));
    commands
}

/// Removes a selection: modules with their cables, frames and notes.
/// Modules inside a deleted frame stay.
pub fn delete_selection(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    selection: &Selection,
) -> Vec<EngineCommand> {
    for &id in &selection.annotations {
        user_state.annotations.remove(id);
    }
    delete_modules(editor, user_state, &selection.nodes)
}

/// Sets a node's parameters back to their defaults, and says whether any
/// changed. Live parameters (a Keyboard's Note and Gate) are the player's,
/// so they stay. The engine hears the new values with the next parameter sync.
pub fn reset_parameters(editor: &mut SynthGraphEditorState, node_id: NodeId) -> bool {
    let Some(template) = editor.graph.nodes.get(node_id)
        .and_then(|node| SynthNodeTemplate::from_module_id(node.user_data.module_id))
    else {
        return false;
    };
    let inputs = port_mapping::parameter_inputs(&editor.graph, node_id);
    let mut changed = false;
    for (input, default) in inputs.into_iter().zip(template.parameter_defaults()) {
        let value = &mut editor.graph.inputs[input].value;
        let before = value.actual_value();
        value.set_actual_value(default);
        changed |= value.actual_value() != before;
    }
    changed
}

/// The top-left corner of some nodes' positions.
pub fn top_left(editor: &SynthGraphEditorState, nodes: &[NodeId]) -> Option<Pos2> {
    nodes
        .iter()
        .filter_map(|&node_id| editor.node_positions.get(node_id).copied())
        .reduce(|a, b| a.min(b))
}

/// Captures a selection as a patch, ready to paste: modules and groups
/// (with everything inside them) and the cables between them, frames and
/// notes. Positions are in unzoomed points from the selection's top-left
/// corner, so the copies keep their layout at any zoom. MIDI mappings stay
/// behind.
pub fn copy_selection(editor: &SynthGraphEditorState, user_state: &SynthGraphState, selection: &Selection) -> Option<Patch> {
    let corner = patch_top_left(editor, user_state, selection)?;
    let position = |node_id| {
        let at = editor.node_positions.get(node_id).map_or(corner, |&p| to_patch(editor, user_state, p));
        ((at - corner).x, (at - corner).y)
    };
    let engine_id = |node_id| user_state.get_engine_node_id(node_id);
    let level = capture_level(&editor.graph, &selection.nodes, None, &engine_id, &position);
    let mut patch = Patch::new(CLIPBOARD_NAME);
    (patch.nodes, patch.connections, patch.groups) = (level.nodes, level.connections, level.groups);
    patch.version = patch.required_version();
    (patch.frames, patch.notes) = user_state.annotations.to_patch(&selection.annotations, -corner.to_vec2());
    let empty = patch.nodes.is_empty() && patch.groups.is_empty() && patch.frames.is_empty() && patch.notes.is_empty();
    (!empty).then_some(patch)
}

/// What a paste or duplicate added.
pub struct Pasted {
    /// The new modules and groups on the level pasted into, in patch order.
    pub nodes: Vec<NodeId>,
    /// The new frames and notes.
    pub annotations: Vec<AnnotationId>,
    /// Engine commands for the new modules and their cables.
    pub commands: Vec<EngineCommand>,
    /// Anything in the patch that couldn't be added.
    pub warnings: Vec<String>,
}

/// Adds a patch's modules and groups, the cables between them, and its
/// frames and notes, with their top-left corner at `at`, on the level the
/// editor shows. Any patch works, not only copied modules: its layout is
/// kept, scaled to the current zoom. Frames and notes only go on the top
/// level of the patch.
pub fn paste(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    patch: &Patch,
    at: Pos2,
) -> Result<Pasted, PatchError> {
    let level = user_state.level;
    let merged = merge_patch(&mut editor.graph, patch, level, &mut || user_state.allocate_group_id())?;
    let zoom = editor.pan_zoom.zoom;
    let at_top = level.is_none();
    let vec = |p: (f32, f32)| Vec2::new(p.0, p.1);
    // Only what lands on this level counts: a group's insides have their own places
    let corner = merged.nodes.iter().filter(|n| !n.nested).map(|n| vec(n.position))
        .chain(merged.parts.iter().filter(|p| !p.nested).map(|p| vec(p.position)))
        .chain(patch.frames.iter().filter(|_| at_top).map(|frame| vec(frame.position)))
        .chain(patch.notes.iter().filter(|_| at_top).map(|note| vec(note.position)))
        .reduce(|a, b| a.min(b))
        .unwrap_or_default();
    let offset = to_patch(editor, user_state, at).to_vec2() - corner;
    // Pads beside others take the next notes (#481), before the engine hears of them
    let landed: Vec<NodeId> = merged.nodes.iter().filter(|n| !n.nested).map(|n| n.graph_id).collect();
    crate::graph::pads_follow_on(&mut editor.graph, &landed);
    let annotations = if at_top {
        user_state.annotations.add_from_patch(&patch.frames, &patch.notes, offset)
    } else {
        Vec::new()
    };

    let mut commands = Vec::new();
    let mut nodes = Vec::new();
    for node in &merged.nodes {
        editor.node_positions.insert(node.graph_id, at + (vec(node.position) - corner) * zoom);
        editor.node_order.push(node.graph_id);
        let engine_node_id = user_state.allocate_engine_node_id(node.graph_id);
        commands.extend(engine_sync::add_module(&editor.graph, node.graph_id, engine_node_id));
        if !node.nested {
            nodes.push(node.graph_id);
        }
    }
    for part in &merged.parts {
        editor.node_positions.insert(part.graph_id, at + (vec(part.position) - corner) * zoom);
        editor.node_order.push(part.graph_id);
        if !part.nested {
            nodes.push(part.graph_id);
        }
    }

    // Pasted Scenes keep the pasted modules, under their new IDs
    crate::persistence::graph_io::remap_scenes(&mut editor.graph, &merged.nodes, |n| user_state.get_engine_node_id(n));

    // The patch's cables only join its own nodes
    commands.extend(engine_sync::sync_cables(&editor.graph, user_state));
    Ok(Pasted { nodes, annotations, commands, warnings: merged.warnings })
}

/// Copies a selection and pastes it a little down and to the right, with
/// the cables between its modules. The clipboard is left alone.
pub fn duplicate(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    selection: &Selection,
) -> Option<Pasted> {
    let patch = copy_selection(editor, user_state, selection)?;
    let corner = from_patch(editor, user_state, patch_top_left(editor, user_state, selection)?);
    let at = corner + DUPLICATE_OFFSET * editor.pan_zoom.zoom;
    paste(editor, user_state, &patch, at).ok()
}

/// "Oscillator" for one module, "group Voice" for a group, "3 modules" for
/// several.
pub fn describe_modules(editor: &SynthGraphEditorState, nodes: &[NodeId]) -> String {
    match nodes {
        [one] => editor.graph.nodes.get(*one).map_or_else(
            || "module".to_string(),
            |node| match node.user_data.kind {
                NodeKind::Group(_) => format!("group {}", node.user_data.display_name),
                _ => SynthNodeTemplate::from_module_id(node.user_data.module_id)
                    .map_or_else(|| "module".to_string(), |t| t.name().to_string()),
            },
        ),
        many => format!("{} modules", many.len()),
    }
}

/// Names a selection for the status bar and undo: "Oscillator", "frame
/// Voice", "3 modules and 1 frame", "2 notes".
pub fn describe(editor: &SynthGraphEditorState, user_state: &SynthGraphState, selection: &Selection) -> String {
    let annotations: Vec<&Annotation> =
        selection.annotations.iter().filter_map(|&id| user_state.annotations.get(id)).collect();
    match (selection.nodes.as_slice(), annotations.as_slice()) {
        (nodes, []) => describe_modules(editor, nodes),
        ([], [one]) => one.describe(),
        (nodes, annotations) => {
            let count = |n: usize, what: &str| match n {
                0 => None,
                1 => Some(format!("1 {what}")),
                n => Some(format!("{n} {what}s")),
            };
            let frames = annotations.iter().filter(|a| matches!(a, Annotation::Frame(_))).count();
            let parts: Vec<String> = [
                count(nodes.len(), "module"),
                count(frames, "frame"),
                count(annotations.len() - frames, "note"),
            ]
            .into_iter()
            .flatten()
            .collect();
            match parts.as_slice() {
                [.., last] if parts.len() > 1 => format!("{} and {last}", parts[..parts.len() - 1].join(", ")),
                _ => parts.concat(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};
    use crate::graph::create_editor_state;
    use crate::persistence::patch_from_json;

    struct Rig {
        editor: SynthGraphEditorState,
        user_state: SynthGraphState,
    }

    impl Rig {
        fn new() -> Self {
            Self { editor: create_editor_state(), user_state: SynthGraphState::new() }
        }

        fn add(&mut self, module_id: &str, at: Pos2) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            add_module(&mut self.editor, &mut self.user_state, template, at).0
        }

        fn connect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
            let graph = &mut self.editor.graph;
            let (output, input) = (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap());
            graph.add_connection(output, input, 0);
        }

        fn param(&self, node_id: NodeId, name: &str) -> f32 {
            let graph = &self.editor.graph;
            graph.get_input(graph[node_id].get_input(name).unwrap()).value.actual_value()
        }

        fn set(&mut self, node_id: NodeId, name: &str, value: f32) {
            let input = self.editor.graph[node_id].get_input(name).unwrap();
            self.editor.graph.inputs[input].value.set_actual_value(value);
        }

        fn module(&self, node_id: NodeId) -> &'static str {
            self.editor.graph[node_id].user_data.module_id
        }

        /// Cables as (from module, to module) pairs.
        fn cables(&self) -> Vec<(NodeId, NodeId)> {
            let graph = &self.editor.graph;
            graph.iter_connections()
                .map(|(input, output)| (graph.get_output(output).node, graph.get_input(input).node))
                .collect()
        }
    }

    /// Oscillator → SVF Filter → Output, with the filter's cutoff turned.
    fn chain(rig: &mut Rig) -> (NodeId, NodeId, NodeId) {
        let osc = rig.add("osc.sine", pos2(100.0, 100.0));
        let filter = rig.add("filter.svf", pos2(300.0, 140.0));
        let out = rig.add("output.audio", pos2(500.0, 100.0));
        rig.connect(osc, "Out", filter, "In");
        rig.connect(filter, "LowPass", out, "Left");
        rig.set(filter, "Cutoff", 2400.0);
        // The engine has heard about the cables, as it would by the end of the frame
        engine_sync::sync_cables(&rig.editor.graph, &mut rig.user_state);
        (osc, filter, out)
    }

    #[test]
    fn delete_takes_the_cables_with_it() {
        let mut rig = Rig::new();
        let (osc, filter, out) = chain(&mut rig);
        let commands = delete_modules(&mut rig.editor, &mut rig.user_state, &[filter]);

        assert!(!rig.editor.graph.nodes.contains_key(filter));
        assert!(rig.cables().is_empty());
        assert_eq!(rig.editor.node_order, vec![osc, out]);
        assert!(rig.user_state.get_engine_node_id(filter).is_none());
        let removes = commands.iter().filter(|c| matches!(c, EngineCommand::RemoveModule { .. })).count();
        let disconnects = commands.iter().filter(|c| matches!(c, EngineCommand::Disconnect { .. })).count();
        // The cable into the filter goes with its module; the one out of it
        // leaves Output's input, which has to be told
        assert_eq!((removes, disconnects), (1, 1));
    }

    #[test]
    fn duplicate_copies_values_and_inner_cables_only() {
        let mut rig = Rig::new();
        let (osc, filter, out) = chain(&mut rig);
        let pasted = duplicate(&mut rig.editor, &mut rig.user_state, &Selection::modules(&[osc, filter])).unwrap();

        let [osc2, filter2] = pasted.nodes[..] else { panic!("expected two copies") };
        assert_eq!((rig.module(osc2), rig.module(filter2)), ("osc.sine", "filter.svf"));
        assert_eq!(rig.param(filter2, "Cutoff"), 2400.0);

        // The cable between the copies came too; the one to Output didn't
        let cables = rig.cables();
        assert!(cables.contains(&(osc2, filter2)));
        assert!(!cables.contains(&(filter2, out)));
        assert_eq!(cables.len(), 3);

        // Layout kept, down and to the right
        let pos = |id| rig.editor.node_positions[id];
        assert_eq!(pos(osc2), pos(osc) + DUPLICATE_OFFSET);
        assert_eq!(pos(filter2) - pos(osc2), pos(filter) - pos(osc));

        // Each copy is a new module with its own engine ID, and its cable goes to the engine
        let adds = pasted.commands.iter().filter(|c| matches!(c, EngineCommand::AddModule { .. })).count();
        let connects = pasted.commands.iter().filter(|c| matches!(c, EngineCommand::Connect { .. })).count();
        assert_eq!((adds, connects), (2, 1));
        assert_ne!(rig.user_state.get_engine_node_id(osc2), rig.user_state.get_engine_node_id(osc));
    }

    #[test]
    fn copies_paste_through_json_at_any_zoom() {
        let mut rig = Rig::new();
        let (osc, filter, _) = chain(&mut rig);
        let patch = copy_selection(&rig.editor, &rig.user_state, &Selection::modules(&[osc, filter])).unwrap();
        // The clipboard carries text
        let patch = patch_from_json(&serde_json::to_string(&patch).unwrap()).unwrap();

        rig.editor.pan_zoom.zoom = 2.0;
        let pasted = paste(&mut rig.editor, &mut rig.user_state, &patch, pos2(10.0, 20.0)).unwrap();
        let [osc2, filter2] = pasted.nodes[..] else { panic!("expected two copies") };
        let pos = |id| rig.editor.node_positions[id];
        assert_eq!(pos(osc2), pos2(10.0, 20.0));
        // Twice as far apart at twice the zoom
        assert_eq!(pos(filter2) - pos(osc2), vec2(400.0, 80.0));
        assert!(pasted.warnings.is_empty());
    }

    #[test]
    fn reset_restores_defaults() {
        let mut rig = Rig::new();
        let (_, filter, _) = chain(&mut rig);
        let fresh = rig.add("filter.svf", pos2(0.0, 0.0));
        assert_ne!(rig.param(filter, "Cutoff"), rig.param(fresh, "Cutoff"));

        assert!(reset_parameters(&mut rig.editor, filter));
        assert_eq!(rig.param(filter, "Cutoff"), rig.param(fresh, "Cutoff"));
        assert!(!reset_parameters(&mut rig.editor, filter), "nothing left to reset");
    }

    #[test]
    fn nothing_to_copy_is_none() {
        let rig = Rig::new();
        assert!(copy_selection(&rig.editor, &rig.user_state, &Selection::default()).is_none());
    }

    /// A frame around the oscillator and filter, with a note under them.
    fn annotate(rig: &mut Rig) -> (AnnotationId, AnnotationId) {
        use crate::graph::annotations::{Frame, Note, Tint};
        let annotations = &mut rig.user_state.annotations;
        let frame = annotations.add(Annotation::Frame(Frame {
            title: "Voice".into(),
            rect: egui::Rect::from_min_size(pos2(80.0, 60.0), vec2(420.0, 300.0)),
            tint: Tint::Blue,
        }));
        let note = annotations.add(Annotation::Note(Note { text: "Saw in".into(), position: pos2(100.0, 380.0), width: 200.0 }));
        (frame, note)
    }

    #[test]
    fn frames_and_notes_copy_with_their_modules_and_keep_their_places() {
        let mut rig = Rig::new();
        let (osc, filter, _) = chain(&mut rig);
        let (frame, note) = annotate(&mut rig);
        let selection = Selection { nodes: vec![osc, filter], annotations: vec![frame, note] };
        let patch = copy_selection(&rig.editor, &rig.user_state, &selection).unwrap();
        // The frame is the top-left corner of the copy
        assert_eq!(patch.frames[0].position, (0.0, 0.0));
        assert_eq!(patch.nodes[0].position, (20.0, 40.0));
        assert_eq!(patch.notes[0].position, (20.0, 320.0));
        let patch = patch_from_json(&serde_json::to_string(&patch).unwrap()).unwrap();

        // Pasted at twice the zoom, the frame still holds the oscillator
        rig.editor.pan_zoom.zoom = 2.0;
        rig.user_state.view_origin = vec2(30.0, -10.0);
        let pasted = paste(&mut rig.editor, &mut rig.user_state, &patch, pos2(1000.0, 1000.0)).unwrap();
        let [frame2, note2] = pasted.annotations[..] else { panic!("expected a frame and a note") };
        let Some(Annotation::Frame(copy)) = rig.user_state.annotations.get(frame2) else { panic!() };
        let osc2 = rig.editor.node_positions[pasted.nodes[0]];
        assert_eq!(from_patch(&rig.editor, &rig.user_state, copy.rect.min), pos2(1000.0, 1000.0));
        assert_eq!(osc2, pos2(1040.0, 1080.0));
        assert_eq!(copy.rect.size(), vec2(420.0, 300.0));
        assert!(matches!(rig.user_state.annotations.get(note2), Some(Annotation::Note(n)) if n.text == "Saw in"));
    }

    #[test]
    fn a_frame_or_note_alone_copies_duplicates_and_deletes() {
        let mut rig = Rig::new();
        let (frame, note) = annotate(&mut rig);
        let selection = Selection { nodes: vec![], annotations: vec![note] };
        assert!(copy_selection(&rig.editor, &rig.user_state, &selection).is_some());

        let pasted = duplicate(&mut rig.editor, &mut rig.user_state, &Selection { nodes: vec![], annotations: vec![frame] }).unwrap();
        let Some(Annotation::Frame(copy)) = rig.user_state.annotations.get(pasted.annotations[0]) else { panic!() };
        assert_eq!(copy.rect.min, pos2(80.0, 60.0) + DUPLICATE_OFFSET);
        assert_eq!(describe(&rig.editor, &rig.user_state, &Selection { nodes: vec![], annotations: vec![frame] }), "frame Voice");

        delete_selection(&mut rig.editor, &mut rig.user_state, &Selection { nodes: vec![], annotations: vec![frame, note] });
        assert!(rig.user_state.annotations.get(frame).is_none());
        assert!(rig.user_state.annotations.get(note).is_none());
    }

    #[test]
    fn mixed_selections_are_described_by_count() {
        let mut rig = Rig::new();
        let (osc, filter, _) = chain(&mut rig);
        let (frame, note) = annotate(&mut rig);
        let describe = |nodes: Vec<NodeId>, annotations: Vec<AnnotationId>| {
            describe(&rig.editor, &rig.user_state, &Selection { nodes, annotations })
        };
        assert_eq!(describe(vec![osc], vec![]), "Oscillator");
        assert_eq!(describe(vec![osc, filter], vec![frame]), "2 modules and 1 frame");
        assert_eq!(describe(vec![osc], vec![frame, note]), "1 module, 1 frame and 1 note");
        assert_eq!(describe(vec![], vec![frame, note]), "1 frame and 1 note");
    }

    #[test]
    fn a_group_duplicates_with_everything_inside_it() {
        let mut rig = Rig::new();
        let (osc, filter, _) = chain(&mut rig);
        let id = rig.user_state.allocate_group_id();
        let bounds = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        let new = groups::group(&mut rig.editor, &[osc, filter], id, "Tone", bounds).unwrap();
        engine_sync::sync_cables(&rig.editor.graph, &mut rig.user_state);
        assert_eq!(describe_modules(&rig.editor, &[new.node]), "group Tone");

        let pasted = duplicate(&mut rig.editor, &mut rig.user_state, &Selection::modules(&[new.node])).unwrap();
        // The copy is a group of its own, on the top level, with its own ID
        let [copy] = pasted.nodes[..] else { panic!("expected the group alone: {:?}", pasted.nodes) };
        let copy_id = rig.editor.graph[copy].user_data.kind.group().unwrap();
        assert_ne!(copy_id, id);
        assert_eq!(rig.editor.graph[copy].label, "Tone");
        assert_eq!(rig.editor.graph[copy].user_data.parent, None);
        let index = GroupIndex::of(&rig.editor.graph);
        assert_eq!(groups::descendants(&rig.editor.graph, &index, copy_id).len(), 4);

        // Two new modules inside it, wired as the originals, and nothing to the output
        let adds = pasted.commands.iter().filter(|c| matches!(c, EngineCommand::AddModule { .. })).count();
        let connects = pasted.commands.iter().filter(|c| matches!(c, EngineCommand::Connect { .. })).count();
        assert_eq!((adds, connects), (2, 1));
    }

    #[test]
    fn a_pad_pinned_to_its_group_stays_pinned_in_the_copy() {
        let mut rig = Rig::new();
        let pad = rig.add(crate::modules::pad::ID, pos2(100.0, 100.0));
        let data = &mut rig.editor.graph[pad].user_data;
        data.pins.insert(groups::PAD_PRESS.into(), 1);
        data.face_labels.insert(groups::PAD_PRESS.into(), "Kick".into());
        let id = rig.user_state.allocate_group_id();
        let bounds = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        let new = groups::group(&mut rig.editor, &[pad], id, "Pads", bounds).unwrap();

        let pasted = duplicate(&mut rig.editor, &mut rig.user_state, &Selection::modules(&[new.node])).unwrap();
        let copy_id = rig.editor.graph[pasted.nodes[0]].user_data.kind.group().unwrap();
        let graph = &rig.editor.graph;
        let shown = groups::face_controls(graph, |_| None, &GroupIndex::of(graph), copy_id);
        let [control] = &shown[..] else { panic!("the copy's pad on its face: {shown:?}") };
        assert_ne!(control.node, pad);
        assert_eq!((control.param.as_str(), control.kind), (groups::PAD_PRESS, groups::Control::Pad));
        assert_eq!(graph[control.node].user_data.face_label(groups::PAD_PRESS), Some("Kick"));
    }

    #[test]
    fn pasting_inside_a_group_lands_there() {
        let mut rig = Rig::new();
        let (osc, filter, out) = chain(&mut rig);
        let id = rig.user_state.allocate_group_id();
        let bounds = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
        groups::group(&mut rig.editor, &[osc, filter], id, "Tone", bounds).unwrap();
        let patch = copy_selection(&rig.editor, &rig.user_state, &Selection::modules(&[out])).unwrap();

        rig.user_state.level = Some(id);
        let pasted = paste(&mut rig.editor, &mut rig.user_state, &patch, pos2(50.0, 50.0)).unwrap();
        assert_eq!(rig.editor.graph[pasted.nodes[0]].user_data.parent, Some(id));
        // A module added from the menu lands there too
        let added = rig.add("osc.sine", pos2(0.0, 0.0));
        assert_eq!(rig.editor.graph[added].user_data.parent, Some(id));
    }
}
