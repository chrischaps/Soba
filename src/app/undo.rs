//! Undo and redo for patch edits.
//!
//! History doesn't hook each kind of edit. It keeps a [`Snapshot`] of the
//! patch as of the last step, and once a gesture is over (no mouse button
//! held) compares the editor with it. Whatever differs becomes one [`Step`]:
//! adding, deleting, connecting, disconnecting, moving and bypassing modules,
//! turning their knobs, grouping and ungrouping them, and adding, moving,
//! resizing, renaming and deleting frames and notes. Nothing is compared
//! while a button is down, so a whole knob turn, node drag or cable repatch
//! is a single step.
//!
//! A step holds both sides of what it changed, so it can be applied in either
//! direction. Applying one edits the editor graph and returns the engine
//! commands for the same edit, so the sound follows. A deleted module comes
//! back under its old engine ID, so its MIDI mappings come back with it.
//! A step remembers the level it was made on (the top of the patch, or
//! inside a group), so undoing it can show it happening.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;
use web_time::Instant;

use egui::{Pos2, Vec2};
use egui_node_graph2::{NodeId, PanZoom};

use crate::engine::{EngineCommand, NodeId as EngineNodeId};
use crate::graph::annotations::{self, Annotation, AnnotationId};
use crate::graph::groups::{self, GroupId, Jack, NodeKind, Side};
use crate::graph::{port_mapping, SynthGraph, SynthGraphEditorState, SynthGraphState, SynthNodeTemplate};
use super::{editing, engine_sync};

/// Most steps kept. The oldest are dropped first.
const MAX_STEPS: usize = 256;

/// Knob changes this close together, to the same knobs, are one step. Knob
/// drags already are; this gathers up anything that edits a value without
/// holding a button, like typing.
const MERGE_WINDOW: Duration = Duration::from_millis(1000);

/// Nodes closer than this to where they were (in unzoomed points) haven't
/// moved. Zooming rescales every position, which leaves rounding behind.
const MOVE_TOLERANCE: f32 = 0.5;

/// How undo knows a node. Modules go by their engine ID, which survives
/// being deleted and brought back, and a group's nodes by the group's ID.
/// Graph IDs don't survive either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum NodeKey {
    Module(EngineNodeId),
    Group(GroupId),
    Inputs(GroupId),
    Outputs(GroupId),
}

/// The key of a graph node, if undo keeps track of it.
fn key_of(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId) -> Option<NodeKey> {
    Some(match graph.nodes.get(node_id)?.user_data.kind {
        NodeKind::Module => NodeKey::Module(user_state.get_engine_node_id(node_id)?),
        NodeKind::Group(id) => NodeKey::Group(id),
        NodeKind::Inputs(id) => NodeKey::Inputs(id),
        NodeKind::Outputs(id) => NodeKey::Outputs(id),
    })
}

/// What a node is, as far as undo cares.
#[derive(Clone, Debug, PartialEq)]
enum Body {
    Module {
        template: SynthNodeTemplate,
        /// Every parameter's value in real units, in parameter order.
        params: Vec<f32>,
        bypassed: bool,
        pins: BTreeMap<String, u8>,
        /// What its pinned controls are called on the groups' faces.
        face_labels: BTreeMap<String, String>,
        /// The file it plays, by sample key.
        file: Option<String>,
        /// Names given to its sections and lanes.
        labels: BTreeMap<String, String>,
    },
    /// A group's own node.
    Group { name: String, inputs: Vec<Jack>, outputs: Vec<Jack> },
    /// A group's Inputs or Outputs.
    Jacks(Vec<Jack>),
}

/// One node, as far as undo cares.
#[derive(Clone, Debug, PartialEq)]
struct NodeState {
    body: Body,
    /// Editor position in unzoomed points, from [`ViewAnchor`].
    position: Vec2,
    /// The group it sits in.
    parent: Option<GroupId>,
}

impl NodeState {
    fn params(&self) -> &[f32] {
        match &self.body {
            Body::Module { params, .. } => params,
            _ => &[],
        }
    }

    /// Everything but the knobs and the position.
    fn same_but_knobs(&self, other: &Self) -> bool {
        self.parent == other.parent
            && match (&self.body, &other.body) {
                (
                    Body::Module { template: t1, bypassed: b1, pins: p1, face_labels: n1, file: f1, labels: l1, .. },
                    Body::Module { template: t2, bypassed: b2, pins: p2, face_labels: n2, file: f2, labels: l2, .. },
                ) => t1 == t2 && b1 == b2 && p1 == p2 && n1 == n2 && f1 == f2 && l1 == l2,
                (a, b) => a == b,
            }
    }

    fn bypassed(&self) -> bool {
        matches!(self.body, Body::Module { bypassed: true, .. })
    }

    fn name(&self) -> String {
        match &self.body {
            Body::Module { template, .. } => template.name().to_string(),
            Body::Group { name, .. } => format!("group {name}"),
            Body::Jacks(_) => "group jacks".to_string(),
        }
    }
}

/// A cable, by node and by port position on each node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cable {
    from: NodeKey,
    /// Position in the source node's outputs.
    output: usize,
    to: NodeKey,
    /// Position in the destination node's inputs.
    input: usize,
}

/// The whole patch, as far as undo cares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    nodes: BTreeMap<NodeKey, NodeState>,
    cables: BTreeSet<Cable>,
    /// Frames and notes, which are already in zoom-free patch space.
    annotations: BTreeMap<AnnotationId, Annotation>,
}

/// Where the editor's node positions put the origin.
///
/// The editor keeps node positions in zoomed points, and zooming moves them
/// all towards or away from the middle of the view. Steps keep positions
/// that don't depend on the zoom, `(position - origin) / zoom`, with the
/// origin moved exactly as zooming moves every node.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewAnchor {
    origin: Vec2,
}

impl ViewAnchor {
    fn unzoomed(&self, position: Pos2, zoom: f32) -> Vec2 {
        (position.to_vec2() - self.origin) / zoom
    }

    fn zoomed(&self, position: Vec2, zoom: f32) -> Pos2 {
        (position * zoom + self.origin).to_pos2()
    }

    /// Follows a zoom from `zoom_before` to the current zoom, made while the
    /// view was panned by `pan_before`. It mirrors the editor scaling node
    /// positions about the middle of the view.
    pub fn follow_zoom(&mut self, zoom_before: f32, pan_before: Vec2, pan_zoom: &PanZoom) {
        if pan_zoom.zoom == zoom_before {
            return;
        }
        let scale = pan_zoom.zoom / zoom_before;
        let half_size = pan_zoom.clip_rect.size() / 2.0;
        self.origin = annotations::follow_zoom(self.origin, scale, half_size, pan_before);
    }
}

impl Snapshot {
    /// Takes the editor's patch. Modules without an engine ID are left out.
    pub fn capture(editor: &SynthGraphEditorState, user_state: &SynthGraphState, anchor: &ViewAnchor) -> Self {
        let graph = &editor.graph;
        let zoom = editor.pan_zoom.zoom;
        let mut snapshot = Self::default();

        for (node_id, node) in graph.nodes.iter() {
            let Some(key) = key_of(graph, user_state, node_id) else { continue };
            let data = &node.user_data;
            let body = match data.kind {
                NodeKind::Module => {
                    let Some(template) = SynthNodeTemplate::from_module_id(data.module_id) else { continue };
                    let params = port_mapping::parameter_inputs(graph, node_id)
                        .into_iter()
                        .map(|input| graph.get_input(input).value.actual_value())
                        .collect();
                    Body::Module {
                        template,
                        params,
                        bypassed: data.bypassed,
                        pins: data.pins.clone(),
                        face_labels: data.face_labels.clone(),
                        file: data.file.clone(),
                        labels: data.labels.clone(),
                    }
                }
                NodeKind::Group(_) => Body::Group {
                    name: data.display_name.clone(),
                    inputs: groups::input_jacks(graph, node_id),
                    outputs: groups::output_jacks(graph, node_id),
                },
                NodeKind::Inputs(_) => Body::Jacks(groups::output_jacks(graph, node_id)),
                NodeKind::Outputs(_) => Body::Jacks(groups::input_jacks(graph, node_id)),
            };
            let position = editor.node_positions.get(node_id).copied().unwrap_or_default();
            snapshot.nodes.insert(key, NodeState {
                body,
                position: anchor.unzoomed(position, zoom),
                parent: data.parent,
            });
        }

        for (input, output) in graph.iter_connections() {
            let (from, to) = (graph.get_output(output).node, graph.get_input(input).node);
            let cable = (|| Some(Cable {
                from: key_of(graph, user_state, from)?,
                output: graph.get_output_index(output)?,
                to: key_of(graph, user_state, to)?,
                input: graph.nodes[to].inputs.iter().position(|(_, id)| *id == input)?,
            }))();
            snapshot.cables.extend(cable);
        }
        snapshot.annotations = user_state.annotations.items().clone();
        snapshot
    }

    /// Records a file a save gave a module, so it isn't mistaken for an edit.
    fn set_file(&mut self, key: EngineNodeId, to: Option<String>) {
        if let Some(NodeState { body: Body::Module { file, .. }, .. }) = self.nodes.get_mut(&NodeKey::Module(key)) {
            *file = to;
        }
    }

    /// Records a value MIDI CC set, so it isn't mistaken for an edit.
    fn set_param(&mut self, key: EngineNodeId, param_index: usize, value: f32) {
        if let Some(NodeState { body: Body::Module { params, .. }, .. }) = self.nodes.get_mut(&NodeKey::Module(key)) {
            if let Some(param) = params.get_mut(param_index) {
                *param = value;
            }
        }
    }
}

/// How one node changed in a step. `None` is a node that isn't there.
#[derive(Clone, Debug)]
struct NodeDiff {
    key: NodeKey,
    before: Option<NodeState>,
    after: Option<NodeState>,
}

/// A cable a step added or removed.
#[derive(Clone, Copy, Debug)]
struct CableDiff {
    cable: Cable,
    added: bool,
}

/// How a frame or note changed in a step. `None` is one that isn't there.
#[derive(Clone, Debug)]
struct AnnotationDiff {
    id: AnnotationId,
    before: Option<Annotation>,
    after: Option<Annotation>,
}

/// One undoable edit, which may touch several modules and cables.
#[derive(Clone, Debug)]
pub struct Step {
    nodes: Vec<NodeDiff>,
    cables: Vec<CableDiff>,
    annotations: Vec<AnnotationDiff>,
    label: String,
    /// The level the edit was made on: the top of the patch, or inside a group.
    level: Option<GroupId>,
}

fn moved(a: Vec2, b: Vec2) -> bool {
    (a - b).length() > MOVE_TOLERANCE
}

/// Positions of the parameters that differ between two states of a node.
fn changed_params<'a>(a: &'a NodeState, b: &'a NodeState) -> impl Iterator<Item = usize> + 'a {
    a.params().iter().zip(b.params()).enumerate().filter(|(_, (x, y))| x != y).map(|(i, _)| i)
}

impl NodeDiff {
    /// Whether the node is the same in substance on both sides.
    fn is_noop(&self) -> bool {
        match (&self.before, &self.after) {
            (Some(a), Some(b)) => {
                !moved(a.position, b.position) && a.same_but_knobs(b) && changed_params(a, b).next().is_none()
            }
            (None, None) => true,
            _ => false,
        }
    }

    /// The parameters a step changed on a node that stayed, if that's all
    /// it changed.
    fn params_only(&self) -> Option<Vec<usize>> {
        match (&self.before, &self.after) {
            (Some(a), Some(b)) if !moved(a.position, b.position) && a.same_but_knobs(b) => {
                Some(changed_params(a, b).collect())
            }
            _ => None,
        }
    }

    fn name(&self) -> String {
        self.before.as_ref().or(self.after.as_ref()).map_or_else(|| "module".to_string(), NodeState::name)
    }
}

impl Step {
    /// Gives a module `file` on both sides of the step.
    fn set_file(&mut self, key: EngineNodeId, to: &Option<String>) {
        for diff in self.nodes.iter_mut().filter(|diff| diff.key == NodeKey::Module(key)) {
            for state in [&mut diff.before, &mut diff.after].into_iter().flatten() {
                if let Body::Module { file, .. } = &mut state.body {
                    file.clone_from(to);
                }
            }
        }
    }

    /// What changed from `before` to `after`, or `None` if nothing did.
    pub fn between(before: &Snapshot, after: &Snapshot) -> Option<Self> {
        let keys: BTreeSet<NodeKey> = before.nodes.keys().chain(after.nodes.keys()).copied().collect();
        let nodes: Vec<NodeDiff> = keys
            .into_iter()
            .map(|key| NodeDiff {
                key,
                before: before.nodes.get(&key).cloned(),
                after: after.nodes.get(&key).cloned(),
            })
            .filter(|diff| !diff.is_noop())
            .collect();
        let cables: Vec<CableDiff> = before.cables.difference(&after.cables)
            .map(|&cable| CableDiff { cable, added: false })
            .chain(after.cables.difference(&before.cables).map(|&cable| CableDiff { cable, added: true }))
            .collect();
        let ids: BTreeSet<AnnotationId> = before.annotations.keys().chain(after.annotations.keys()).copied().collect();
        let annotations: Vec<AnnotationDiff> = ids
            .into_iter()
            .map(|id| AnnotationDiff {
                id,
                before: before.annotations.get(&id).cloned(),
                after: after.annotations.get(&id).cloned(),
            })
            .filter(|diff| diff.before != diff.after)
            .collect();

        if nodes.is_empty() && cables.is_empty() && annotations.is_empty() {
            return None;
        }
        let label = if annotations.is_empty() {
            describe(&nodes, &cables, before, after)
        } else {
            describe_annotations(&annotations, &nodes, &cables)
        };
        Some(Self { nodes, cables, annotations, label, level: None })
    }

    /// What the step did, for the Edit buttons and status bar,
    /// e.g. "Move Oscillator" or "Set SVF Filter Cutoff".
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The step that undoes this one.
    fn inverse(&self) -> Self {
        Self {
            nodes: self.nodes.iter()
                .map(|diff| NodeDiff { key: diff.key, before: diff.after.clone(), after: diff.before.clone() })
                .collect(),
            cables: self.cables.iter().map(|diff| CableDiff { added: !diff.added, ..*diff }).collect(),
            annotations: self.annotations.iter()
                .map(|diff| AnnotationDiff { id: diff.id, before: diff.after.clone(), after: diff.before.clone() })
                .collect(),
            label: self.label.clone(),
            level: self.level,
        }
    }

    /// Folds `next` into this step, if both only turned the same knobs.
    fn absorb(&mut self, next: &Step) -> bool {
        let same_knobs = self.cables.is_empty()
            && next.cables.is_empty()
            && self.annotations.is_empty()
            && next.annotations.is_empty()
            && self.nodes.len() == next.nodes.len()
            && self.nodes.iter().zip(&next.nodes).all(|(a, b)| {
                a.key == b.key && a.params_only().is_some() && a.params_only() == b.params_only()
            });
        if !same_knobs {
            return false;
        }
        for (mine, theirs) in self.nodes.iter_mut().zip(&next.nodes) {
            mine.after = theirs.after.clone();
        }
        true
    }

    /// Makes the editor match the `after` side of the step, and returns the
    /// engine commands that do the same to the audio graph.
    fn apply(&self, editor: &mut SynthGraphEditorState, user_state: &mut SynthGraphState, anchor: &ViewAnchor) -> Vec<EngineCommand> {
        let mut commands = Vec::new();
        let zoom = editor.pan_zoom.zoom;
        let mut graph_ids: HashMap<NodeKey, NodeId> = editor.graph.nodes.keys()
            .filter_map(|node_id| Some((key_of(&editor.graph, user_state, node_id)?, node_id)))
            .collect();

        // Cables come out first, so neither a removed node nor a cable
        // about to take their input is still attached to them
        for diff in self.cables.iter().filter(|d| !d.added) {
            let graph = &mut editor.graph;
            if let Some((output, input)) = cable_ports(graph, &graph_ids, diff.cable) {
                graph.remove_connection(input, output);
            }
        }

        // Nodes that go
        for diff in self.nodes.iter().filter(|d| d.after.is_none()) {
            let Some(node_id) = graph_ids.remove(&diff.key) else { continue };
            commands.extend(editing::remove_module(editor, user_state, node_id));
        }

        // Nodes that come (back), modules under the engine ID they had
        for diff in self.nodes.iter().filter(|d| d.before.is_none()) {
            let Some(state) = &diff.after else { continue };
            let position = anchor.zoomed(state.position, zoom);
            let node_id = match (&state.body, diff.key) {
                (Body::Module { template, bypassed, pins, face_labels, file, labels, .. }, NodeKey::Module(engine_id)) => {
                    let node_id = editing::place_node(editor, user_state, *template, position);
                    let data = &mut editor.graph[node_id].user_data;
                    data.bypassed = *bypassed;
                    data.pins = pins.clone();
                    data.face_labels = face_labels.clone();
                    data.labels = labels.clone();
                    // Its recording follows from the file, as the app syncs samples
                    data.file = file.clone();
                    user_state.assign_engine_node_id(node_id, engine_id);
                    commands.extend(engine_sync::add_module(&editor.graph, node_id, engine_id));

                    // Every value goes to the new module, since it starts at defaults
                    let all = 0..state.params().len();
                    commands.extend(set_params(editor, node_id, engine_id, *template, state.params(), all));
                    node_id
                }
                (Body::Group { name, inputs, outputs }, NodeKey::Group(id)) => {
                    user_state.claim_group_id(id);
                    groups::add_group_node(&mut editor.graph, id, name, state.parent, inputs, outputs)
                }
                (Body::Jacks(jacks), NodeKey::Inputs(id)) => groups::add_inputs_node(&mut editor.graph, id, jacks),
                (Body::Jacks(jacks), NodeKey::Outputs(id)) => groups::add_outputs_node(&mut editor.graph, id, jacks),
                _ => continue,
            };
            if !editor.node_order.contains(&node_id) {
                editor.node_order.push(node_id);
            }
            editor.node_positions.insert(node_id, position);
            editor.graph[node_id].user_data.parent = state.parent;
            graph_ids.insert(diff.key, node_id);
        }

        // Nodes that stay but changed
        for diff in &self.nodes {
            let (Some(before), Some(after)) = (&diff.before, &diff.after) else { continue };
            let Some(&node_id) = graph_ids.get(&diff.key) else { continue };
            if let (Body::Module { template, .. }, NodeKey::Module(engine_id)) = (&after.body, diff.key) {
                let changed: Vec<usize> = changed_params(before, after).collect();
                commands.extend(set_params(editor, node_id, engine_id, *template, after.params(), changed));
                if before.bypassed() != after.bypassed() {
                    editor.graph[node_id].user_data.bypassed = after.bypassed();
                    commands.push(EngineCommand::SetBypass { node_id: engine_id, bypassed: after.bypassed() });
                }
            }
            match &after.body {
                Body::Module { pins, face_labels, file, labels, .. } => {
                    let data = &mut editor.graph[node_id].user_data;
                    data.pins = pins.clone();
                    data.face_labels = face_labels.clone();
                    data.file = file.clone();
                    data.labels = labels.clone();
                }
                Body::Group { name, inputs, outputs } => {
                    groups::rename(&mut editor.graph, node_id, name);
                    groups::set_jacks(&mut editor.graph, node_id, Side::Input, inputs);
                    groups::set_jacks(&mut editor.graph, node_id, Side::Output, outputs);
                }
                // Inputs carries the input jacks as outputs, Outputs the reverse
                Body::Jacks(jacks) => match diff.key {
                    NodeKey::Inputs(_) => groups::set_jacks(&mut editor.graph, node_id, Side::Output, jacks),
                    NodeKey::Outputs(_) => groups::set_jacks(&mut editor.graph, node_id, Side::Input, jacks),
                    _ => {}
                },
            }
            editor.graph[node_id].user_data.parent = after.parent;
            if moved(before.position, after.position) {
                editor.node_positions.insert(node_id, anchor.zoomed(after.position, zoom));
            }
        }

        // Cables go in last, once both ends exist
        for diff in self.cables.iter().filter(|d| d.added) {
            let graph = &mut editor.graph;
            if let Some((output, input)) = cable_ports(graph, &graph_ids, diff.cable) {
                graph.add_connection(output, input, 0);
            }
        }
        // Whatever the cables add up to now, module to module, is what the
        // engine should have
        commands.extend(engine_sync::sync_cables(&editor.graph, user_state));

        // Frames and notes make no sound, so the engine hears nothing of them
        let annotations = &mut user_state.annotations;
        annotations.editing = None;
        for diff in &self.annotations {
            match &diff.after {
                Some(after) => annotations.restore(diff.id, after.clone()),
                None => {
                    annotations.remove(diff.id);
                }
            }
        }
        commands
    }
}

/// The graph ports at the ends of a cable, if both nodes and ports exist.
fn cable_ports(
    graph: &SynthGraph,
    graph_ids: &HashMap<NodeKey, NodeId>,
    cable: Cable,
) -> Option<(egui_node_graph2::OutputId, egui_node_graph2::InputId)> {
    let from = graph.nodes.get(*graph_ids.get(&cable.from)?)?;
    let to = graph.nodes.get(*graph_ids.get(&cable.to)?)?;
    Some((from.outputs.get(cable.output)?.1, to.inputs.get(cable.input)?.1))
}

/// Sets some of a module's parameters to `values`, and returns the
/// SetParameter commands for them. Live parameters (a Keyboard's Note and
/// Gate) are the player's, so they're left alone.
fn set_params(
    editor: &mut SynthGraphEditorState,
    node_id: NodeId,
    key: EngineNodeId,
    template: SynthNodeTemplate,
    values: &[f32],
    indices: impl IntoIterator<Item = usize>,
) -> Vec<EngineCommand> {
    let inputs = port_mapping::parameter_inputs(&editor.graph, node_id);
    let live = template.live_parameter_count();
    indices
        .into_iter()
        .filter(|&i| i >= live)
        .filter_map(|param_index| {
            let input = editor.graph.inputs.get_mut(*inputs.get(param_index)?)?;
            input.value.set_actual_value(*values.get(param_index)?);
            Some(EngineCommand::SetParameter { node_id: key, param_index, value: input.value.actual_value() })
        })
        .collect()
}

/// Names a step after what it did.
fn describe(nodes: &[NodeDiff], cables: &[CableDiff], before: &Snapshot, after: &Snapshot) -> String {
    let added: Vec<&NodeDiff> = nodes.iter().filter(|d| d.before.is_none()).collect();
    let removed: Vec<&NodeDiff> = nodes.iter().filter(|d| d.after.is_none()).collect();
    let changed: Vec<&NodeDiff> = nodes.iter().filter(|d| d.before.is_some() && d.after.is_some()).collect();
    let modules = |n: usize| if n == 1 { "module".to_string() } else { format!("{n} modules") };

    if !added.is_empty() || !removed.is_empty() {
        return match (added.as_slice(), removed.as_slice()) {
            ([one], []) => format!("Add {}", one.name()),
            (many, []) => format!("Add {}", modules(many.len())),
            ([], [one]) => format!("Delete {}", one.name()),
            ([], many) => format!("Delete {}", modules(many.len())),
            _ => "Edit patch".to_string(),
        };
    }

    if changed.is_empty() {
        let name = |key: NodeKey| {
            after.nodes.get(&key).or(before.nodes.get(&key)).map_or_else(|| "module".to_string(), NodeState::name)
        };
        let ends = |c: &Cable| format!("{} → {}", name(c.from), name(c.to));
        let made: Vec<&Cable> = cables.iter().filter(|d| d.added).map(|d| &d.cable).collect();
        let cut: Vec<&Cable> = cables.iter().filter(|d| !d.added).map(|d| &d.cable).collect();
        return match (made.as_slice(), cut.as_slice()) {
            ([one], []) => format!("Connect {}", ends(one)),
            ([], [one]) => format!("Disconnect {}", ends(one)),
            ([one], [_]) => format!("Repatch {}", ends(one)),
            _ => "Repatch cables".to_string(),
        };
    }
    if !cables.is_empty() {
        return "Edit patch".to_string();
    }
    if let [one] = changed.as_slice() {
        if let (Some(a), Some(b)) = (&one.before, &one.after) {
            if let (Body::Group { name: was, .. }, Body::Group { name: is, .. }) = (&a.body, &b.body) {
                if was != is {
                    return format!("Rename group {is}");
                }
            }
        }
    }

    let pairs = || changed.iter().filter_map(|d| Some((d, d.before.as_ref()?, d.after.as_ref()?)));
    if pairs().all(|(_, a, b)| changed_params(a, b).next().is_none() && a.same_but_knobs(b)) {
        return match changed.as_slice() {
            [one] => format!("Move {}", one.name()),
            many => format!("Move {}", modules(many.len())),
        };
    }
    let only = |same: fn(&NodeState, &NodeState) -> bool| {
        pairs().all(|(_, a, b)| changed_params(a, b).next().is_none() && !moved(a.position, b.position) && same(a, b))
    };
    if only(|a, b| {
        a.parent == b.parent && pins(a) == pins(b) && a.bypassed() == b.bypassed() && file(a) == file(b) && labels(a) == labels(b)
            && face_labels(a) != face_labels(b)
    }) {
        return match changed.as_slice() {
            [one] => format!("Name {} control on face", one.name()),
            _ => "Name face controls".to_string(),
        };
    }
    if only(|a, b| a.parent == b.parent && pins(a) == pins(b) && a.bypassed() == b.bypassed() && file(a) == file(b) && labels(a) != labels(b)) {
        return match changed.as_slice() {
            [one] => format!("Rename in {}", one.name()),
            many => format!("Rename in {}", modules(many.len())),
        };
    }
    if only(|a, b| a.parent == b.parent && pins(a) == pins(b) && a.bypassed() == b.bypassed() && file(a) != file(b)) {
        return match changed.as_slice() {
            [one] => format!("Load sample into {}", one.name()),
            many => format!("Load samples into {}", modules(many.len())),
        };
    }
    if only(|a, b| a.parent == b.parent && pins(a) == pins(b) && file(a) == file(b)) {
        return match (changed.as_slice(), pairs().next()) {
            ([one], Some((_, _, b))) if b.bypassed() => format!("Bypass {}", one.name()),
            ([one], Some(_)) => format!("Switch on {}", one.name()),
            _ => format!("Bypass {}", modules(changed.len())),
        };
    }
    if only(|a, b| a.parent == b.parent && a.bypassed() == b.bypassed() && file(a) == file(b) && labels(a) == labels(b)) {
        return match pairs().next() {
            Some((one, a, b)) if changed.len() == 1 => {
                let (was, is) = (pins(a).len(), pins(b).len());
                if is >= was {
                    format!("Pin {} knob", one.name())
                } else {
                    format!("Unpin {} knob", one.name())
                }
            }
            _ => "Pin knobs".to_string(),
        };
    }
    if let ([one], Some((_, a, b))) = (changed.as_slice(), pairs().next()) {
        let params: Vec<usize> = changed_params(a, b).collect();
        if let ([param], false, true) = (params.as_slice(), moved(a.position, b.position), a.same_but_knobs(b)) {
            if let Body::Module { template, .. } = &b.body {
                if let Some(param_name) = template.parameter_names().get(*param) {
                    return format!("Set {} {}", one.name(), param_name);
                }
            }
        }
        return format!("Change {}", one.name());
    }
    "Change modules".to_string()
}

/// A module's pinned knobs.
fn pins(state: &NodeState) -> BTreeMap<String, u8> {
    match &state.body {
        Body::Module { pins, .. } => pins.clone(),
        _ => BTreeMap::new(),
    }
}

/// What a module's pinned controls are called on the groups' faces.
fn face_labels(state: &NodeState) -> Option<&BTreeMap<String, String>> {
    match &state.body {
        Body::Module { face_labels, .. } => Some(face_labels),
        _ => None,
    }
}

/// The names given to a module's sections and lanes.
fn labels(state: &NodeState) -> Option<&BTreeMap<String, String>> {
    match &state.body {
        Body::Module { labels, .. } => Some(labels),
        _ => None,
    }
}

/// The file a module plays.
fn file(state: &NodeState) -> Option<&str> {
    match &state.body {
        Body::Module { file, .. } => file.as_deref(),
        _ => None,
    }
}

/// Names a step that changed frames or notes: "Move frame Voice" (with the
/// modules inside it), "Resize frame Voice", "Edit note", "Add note".
fn describe_annotations(annotations: &[AnnotationDiff], nodes: &[NodeDiff], cables: &[CableDiff]) -> String {
    // Modules moving along with a frame dragged by its title
    let modules_only_moved = cables.is_empty()
        && nodes.iter().all(|d| match (&d.before, &d.after) {
            (Some(a), Some(b)) => a.same_but_knobs(b) && changed_params(a, b).next().is_none(),
            _ => false,
        });
    if !modules_only_moved {
        return "Edit patch".to_string();
    }
    let [diff] = annotations else {
        let what = format!("{} frames and notes", annotations.len());
        return if annotations.iter().all(|d| d.before.is_none()) {
            format!("Add {what}")
        } else if annotations.iter().all(|d| d.after.is_none()) {
            format!("Delete {what}")
        } else {
            format!("Change {what}")
        };
    };
    match (&diff.before, &diff.after) {
        (None, Some(after)) => format!("Add {}", after.describe()),
        (Some(before), None) => format!("Delete {}", before.describe()),
        (Some(Annotation::Frame(a)), Some(after @ Annotation::Frame(b))) => {
            let what = after.describe();
            if a.title != b.title {
                format!("Rename {what}")
            } else if a.tint != b.tint {
                format!("Color {what}")
            } else if a.rect.size() != b.rect.size() {
                format!("Resize {what}")
            } else {
                format!("Move {what}")
            }
        }
        (Some(Annotation::Note(a)), Some(Annotation::Note(b))) => {
            if a.text != b.text {
                "Edit note".to_string()
            } else if a.width != b.width {
                "Resize note".to_string()
            } else {
                "Move note".to_string()
            }
        }
        _ => "Edit patch".to_string(),
    }
}

/// What undo or redo did: the step's label, the engine commands that make
/// the audio graph match, and the level the edit was made on.
pub struct Applied {
    pub label: String,
    pub commands: Vec<EngineCommand>,
    pub level: Option<GroupId>,
}

/// The undo and redo stacks, and the patch as of the last step.
#[derive(Default)]
pub struct History {
    baseline: Snapshot,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// When the last step was recorded, while it can still absorb more knob
    /// changes. Undo and redo close it.
    open_since: Option<Instant>,
    anchor: ViewAnchor,
    /// A name for the next step, from the command that's making it.
    next_label: Option<String>,
    /// The patch as it was last opened or saved, to tell whether it has
    /// unsaved changes. Undoing back to it counts as no changes.
    saved: Snapshot,
    /// The patch has never been saved as it stands, e.g. one recovered
    /// from an autosave, so it has unsaved changes whatever the history says.
    never_saved: bool,
}

impl History {
    /// Forgets every step and starts over from the editor's patch, e.g.
    /// after loading one.
    pub fn reset(&mut self, editor: &SynthGraphEditorState, user_state: &SynthGraphState) {
        self.undo.clear();
        self.redo.clear();
        self.open_since = None;
        self.next_label = None;
        self.baseline = Snapshot::capture(editor, user_state, &self.anchor);
        self.mark_saved();
    }

    /// Notes that the patch, as of the last step, was just saved.
    pub fn mark_saved(&mut self) {
        self.saved = self.baseline.clone();
        self.never_saved = false;
    }

    /// Notes that the patch isn't saved anywhere as it stands.
    pub fn mark_unsaved(&mut self) {
        self.never_saved = true;
    }

    /// Whether the patch, as of the last step, differs from what was last
    /// opened or saved.
    pub fn has_unsaved_changes(&self) -> bool {
        self.never_saved || Step::between(&self.saved, &self.baseline).is_some()
    }

    /// Starts tracking zoom afresh, for when the editor's pan and zoom are
    /// reset to their defaults.
    pub fn reset_view(&mut self) {
        self.anchor = ViewAnchor::default();
    }

    /// Where the patch's (0, 0) is, relative to the editor's pan, in the
    /// same zoomed points as node positions. The background grid hangs off it.
    pub fn view_origin(&self) -> Vec2 {
        self.anchor.origin
    }

    /// A node's editor position as a zoom-free patch position: what patches
    /// save, and what frames and notes are kept in.
    pub fn to_patch(&self, position: Pos2, zoom: f32) -> Pos2 {
        self.anchor.unzoomed(position, zoom).to_pos2()
    }

    /// The editor position of a patch position, at the given zoom.
    pub fn from_patch(&self, position: Pos2, zoom: f32) -> Pos2 {
        self.anchor.zoomed(position.to_vec2(), zoom)
    }

    /// Follows a zoom of the editor. See [`ViewAnchor::follow_zoom`].
    pub fn follow_zoom(&mut self, zoom_before: f32, pan_before: Vec2, pan_zoom: &PanZoom) {
        self.anchor.follow_zoom(zoom_before, pan_before, pan_zoom);
    }

    /// Records anything that changed since the last step as a new step.
    /// Call it once a frame, after every edit. While `gesture_held` (a mouse
    /// button is down) it waits, so the gesture becomes one step.
    pub fn record(&mut self, editor: &SynthGraphEditorState, user_state: &SynthGraphState, gesture_held: bool, now: Instant) {
        if gesture_held {
            return;
        }
        let label = self.next_label.take();
        let current = Snapshot::capture(editor, user_state, &self.anchor);
        let Some(mut step) = Step::between(&self.baseline, &current) else {
            return;
        };
        step.level = user_state.level;
        self.baseline = current;
        self.redo.clear();

        // A named command is a step of its own: nothing merges into or out of it
        if let Some(label) = label {
            step.label = label;
            self.push(step);
            self.open_since = None;
            return;
        }
        let merges = self.open_since.is_some_and(|since| now.duration_since(since) < MERGE_WINDOW);
        if !(merges && self.undo.last_mut().is_some_and(|last| last.absorb(&step))) {
            self.push(step);
        }
        self.open_since = Some(now);
    }

    fn push(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// Names the step the current frame's edits will make, e.g. "Paste 3
    /// modules", in place of the name it would get from what changed.
    pub fn name_next(&mut self, label: impl Into<String>) {
        self.next_label = Some(label.into());
    }

    /// Notes a value set by MIDI CC: playing a controller isn't an edit.
    pub fn absorb_param(&mut self, key: EngineNodeId, param_index: usize, value: f32) {
        self.baseline.set_param(key, param_index, value);
    }

    /// Notes a file a save gave a module (a Looper's loop, written beside
    /// the patch). Keeping the loop isn't an edit to undo, so every step
    /// takes the module as having it: undoing one never puts back a file
    /// the loop is no longer in.
    pub fn absorb_file(&mut self, key: EngineNodeId, file: Option<String>) {
        self.baseline.set_file(key, file.clone());
        for step in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            step.set_file(key, &file);
        }
    }

    /// Undoes the last step, if there is one.
    pub fn undo(&mut self, editor: &mut SynthGraphEditorState, user_state: &mut SynthGraphState) -> Option<Applied> {
        // A gesture still in progress is finished first, so it's what gets undone
        self.record(editor, user_state, false, Instant::now());
        let step = self.undo.pop()?;
        let applied = self.replay(&step.inverse(), editor, user_state);
        self.redo.push(step);
        Some(applied)
    }

    /// Redoes the last undone step, if there is one.
    pub fn redo(&mut self, editor: &mut SynthGraphEditorState, user_state: &mut SynthGraphState) -> Option<Applied> {
        self.record(editor, user_state, false, Instant::now());
        let step = self.redo.pop()?;
        let applied = self.replay(&step, editor, user_state);
        self.undo.push(step);
        Some(applied)
    }

    fn replay(&mut self, step: &Step, editor: &mut SynthGraphEditorState, user_state: &mut SynthGraphState) -> Applied {
        let commands = step.apply(editor, user_state, &self.anchor);
        self.baseline = Snapshot::capture(editor, user_state, &self.anchor);
        self.open_since = None;
        Applied { label: step.label.clone(), commands, level: step.level }
    }

    /// What Undo would undo.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(Step::label)
    }

    /// What Redo would redo.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(Step::label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};
    use egui_node_graph2::GraphEditorState;
    use crate::graph::annotations::Annotation;
    use crate::graph::groups;

    /// An editor without a window: the graph, its user state and undo
    /// history, edited the way the editor and its responses edit them.
    struct Rig {
        editor: SynthGraphEditorState,
        user_state: SynthGraphState,
        history: History,
        /// The clock steps are recorded at
        now: Instant,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Self {
                editor: GraphEditorState::new(1.0),
                user_state: SynthGraphState::new(),
                history: History::default(),
                now: Instant::now(),
            };
            rig.history.reset(&rig.editor, &rig.user_state);
            rig
        }

        /// Adds a module, as the add-module menu does.
        fn add(&mut self, module_id: &str, at: Pos2) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            editing::add_module(&mut self.editor, &mut self.user_state, template, at).0
        }

        /// Deletes a module, as its close button does.
        fn delete(&mut self, node_id: NodeId) {
            self.editor.graph.remove_node(node_id);
            self.editor.node_positions.remove(node_id);
            self.editor.node_order.retain(|id| *id != node_id);
            self.user_state.remove_node(node_id);
        }

        fn ports(&self, from: NodeId, output: &str, to: NodeId, input: &str) -> (egui_node_graph2::OutputId, egui_node_graph2::InputId) {
            let graph = &self.editor.graph;
            (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap())
        }

        /// Patches a cable, replacing any already in the input.
        fn connect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
            let (output, input) = self.ports(from, output, to, input);
            self.editor.graph.add_connection(output, input, 0);
        }

        fn disconnect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
            let (output, input) = self.ports(from, output, to, input);
            assert!(self.editor.graph.remove_connection(input, output));
        }

        fn drag(&mut self, node_id: NodeId, by: Vec2) {
            self.editor.node_positions[node_id] += by;
        }

        fn param(&self, node_id: NodeId, name: &str) -> f32 {
            self.editor.graph.get_input(self.editor.graph[node_id].get_input(name).unwrap()).value.actual_value()
        }

        fn set(&mut self, node_id: NodeId, name: &str, value: f32) {
            let input = self.editor.graph[node_id].get_input(name).unwrap();
            self.editor.graph.inputs[input].value.set_actual_value(value);
        }

        fn engine_id(&self, node_id: NodeId) -> EngineNodeId {
            self.user_state.get_engine_node_id(node_id).unwrap()
        }

        /// The graph node now standing for an engine ID.
        fn node(&self, key: EngineNodeId) -> NodeId {
            self.user_state.node_id_map.iter().find(|(_, &k)| k == key).map(|(&id, _)| id).unwrap()
        }

        /// Ends a gesture, a second after the last. The engine hears about
        /// the frame's cables first, as it does at the end of every frame.
        fn record(&mut self) {
            self.now += Duration::from_secs(1);
            engine_sync::sync_cables(&self.editor.graph, &mut self.user_state);
            self.history.record(&self.editor, &self.user_state, false, self.now);
        }

        /// A frame in the middle of a gesture.
        fn record_held(&mut self) {
            self.now += Duration::from_millis(16);
            engine_sync::sync_cables(&self.editor.graph, &mut self.user_state);
            self.history.record(&self.editor, &self.user_state, true, self.now);
        }

        fn snapshot(&self) -> Snapshot {
            Snapshot::capture(&self.editor, &self.user_state, &self.history.anchor)
        }

        fn undo(&mut self) -> Applied {
            self.history.undo(&mut self.editor, &mut self.user_state).expect("something to undo")
        }

        fn redo(&mut self) -> Applied {
            self.history.redo(&mut self.editor, &mut self.user_state).expect("something to redo")
        }

        /// Zooms about the middle of the view, as scrolling does.
        fn zoom(&mut self, scale: f32) {
            let pan_zoom = &mut self.editor.pan_zoom;
            pan_zoom.clip_rect = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
            let (zoom_before, pan) = (pan_zoom.zoom, pan_zoom.pan);
            pan_zoom.zoom *= scale;
            let half_size = pan_zoom.clip_rect.size() / 2.0;
            for (_, pos) in self.editor.node_positions.iter_mut() {
                *pos = ((pos.to_vec2() - half_size + pan) * scale + half_size - pan).to_pos2();
            }
            self.history.follow_zoom(zoom_before, pan, &self.editor.pan_zoom);
        }
    }

    #[test]
    fn grid_clicks_undo_one_at_a_time() {
        use crate::modules::trigger_sequencer::Step;

        let mut rig = Rig::new();
        let seq = rig.add("seq.trigger", pos2(100.0, 100.0));
        rig.record();
        let kick = Step { on: true, ..Step::DEFAULT }.encode();

        // Three quick clicks, as the grid sends them: two steps, and the
        // first one again, each a frame apart and named
        for name in ["Step A1 01", "Step A1 05", "Step A1 01"] {
            let on = rig.param(seq, name) < 0.0;
            rig.set(seq, name, if on { kick } else { Step::DEFAULT.encode() });
            rig.history.name_next(format!("Toggle {name}"));
            rig.now += Duration::from_millis(16);
            rig.history.record(&rig.editor, &rig.user_state, false, rig.now);
        }
        assert!(rig.param(seq, "Step A1 01") < 0.0);
        assert!(rig.param(seq, "Step A1 05") > 0.0);

        // Each comes back on its own
        assert_eq!(rig.undo().label, "Toggle Step A1 01");
        assert!(rig.param(seq, "Step A1 01") > 0.0);
        assert_eq!(rig.undo().label, "Toggle Step A1 05");
        assert!(rig.param(seq, "Step A1 05") < 0.0);
        assert_eq!(rig.undo().label, "Toggle Step A1 01");
        assert!(rig.param(seq, "Step A1 01") < 0.0);
    }

    fn same(a: &Snapshot, b: &Snapshot) -> bool {
        Step::between(a, b).is_none()
    }

    /// Records an edit, then checks undo puts back what was there before
    /// it, and redo what was there after.
    fn round_trip(rig: &mut Rig, edit: impl FnOnce(&mut Rig)) -> (Applied, Applied) {
        let before = rig.snapshot();
        edit(rig);
        rig.record();
        let after = rig.snapshot();
        assert!(!same(&before, &after), "the edit changed nothing");

        let undone = rig.undo();
        assert!(same(&rig.snapshot(), &before), "undo left {:?}", Step::between(&before, &rig.snapshot()));
        let redone = rig.redo();
        assert!(same(&rig.snapshot(), &after), "redo left {:?}", Step::between(&after, &rig.snapshot()));
        (undone, redone)
    }

    /// An oscillator into a filter into the output.
    fn voice(rig: &mut Rig) -> (NodeId, NodeId, NodeId) {
        let osc = rig.add("osc.sine", pos2(100.0, 100.0));
        let filter = rig.add("filter.svf", pos2(400.0, 100.0));
        let out = rig.add("output.audio", pos2(700.0, 100.0));
        rig.connect(osc, "Out", filter, "In");
        rig.connect(filter, "LowPass", out, "Left");
        rig.record();
        (osc, filter, out)
    }

    fn count(commands: &[EngineCommand], wanted: impl Fn(&EngineCommand) -> bool) -> usize {
        commands.iter().filter(|c| wanted(c)).count()
    }

    #[test]
    fn test_unsaved_changes_follow_the_saved_patch() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        rig.history.mark_saved();
        assert!(!rig.history.has_unsaved_changes());

        rig.set(filter, "Cutoff", 440.0);
        rig.record();
        assert!(rig.history.has_unsaved_changes());

        // Undoing back to what was saved leaves nothing to save; redoing does
        rig.undo();
        assert!(!rig.history.has_unsaved_changes());
        rig.redo();
        assert!(rig.history.has_unsaved_changes());

        rig.history.mark_saved();
        assert!(!rig.history.has_unsaved_changes());
        // Zooming moves every node on screen, but that isn't an edit
        rig.zoom(1.7);
        rig.record();
        assert!(!rig.history.has_unsaved_changes());
    }

    #[test]
    fn test_a_recovered_patch_is_unsaved_until_saved() {
        let mut rig = Rig::new();
        voice(&mut rig);
        rig.history.reset(&rig.editor, &rig.user_state);
        assert!(!rig.history.has_unsaved_changes());
        rig.history.mark_unsaved();
        assert!(rig.history.has_unsaved_changes());
        rig.history.mark_saved();
        assert!(!rig.history.has_unsaved_changes());
    }

    #[test]
    fn test_add_round_trips() {
        let mut rig = Rig::new();
        let (undone, redone) = round_trip(&mut rig, |rig| {
            rig.add("osc.sine", pos2(50.0, 60.0));
        });
        assert_eq!(undone.label, "Add Oscillator");
        assert!(matches!(undone.commands[..], [EngineCommand::RemoveModule { node_id: 0 }]));
        // It comes back as the same engine node, where it was
        assert!(matches!(redone.commands[0], EngineCommand::AddModule { node_id: 0, module_id: "osc.sine" }));
        assert_eq!(rig.editor.node_positions[rig.node(0)], pos2(50.0, 60.0));
    }

    #[test]
    fn test_delete_brings_back_values_cables_and_engine_id() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        rig.set(filter, "Cutoff", 440.0);
        rig.record();
        let key = rig.engine_id(filter);

        let (undone, redone) = round_trip(&mut rig, |rig| rig.delete(filter));
        assert_eq!(undone.label, "Delete SVF Filter");

        let commands = &undone.commands;
        assert!(matches!(commands[0], EngineCommand::AddModule { node_id, .. } if node_id == key));
        // A fresh module starts at defaults, so the edited value is sent
        assert_eq!(count(commands, |c| matches!(c,
            EngineCommand::SetParameter { node_id, param_index: 0, value } if *node_id == key && *value == 440.0)), 1);
        assert_eq!(count(commands, |c| matches!(c, EngineCommand::Connect { .. })), 2);
        assert_eq!(count(&redone.commands, |c| matches!(c, EngineCommand::RemoveModule { node_id } if *node_id == key)), 1);

        rig.undo();
        assert_eq!(rig.param(rig.node(key), "Cutoff"), 440.0);
    }

    #[test]
    fn test_connect_round_trips() {
        let mut rig = Rig::new();
        let (osc, filter, out) = voice(&mut rig);
        let (undone, redone) = round_trip(&mut rig, |rig| rig.connect(osc, "Sub", out, "Right"));
        assert_eq!(undone.label, "Connect Oscillator → Audio Output");
        assert_eq!(count(&undone.commands, |c| matches!(c, EngineCommand::Disconnect { .. })), 1);
        assert_eq!(count(&redone.commands, |c| matches!(c, EngineCommand::Connect { .. })), 1);

        // A cable into an exposed knob's jack makes the knob follow it
        let (undone, redone) = round_trip(&mut rig, |rig| rig.connect(osc, "Out", filter, "Cutoff"));
        assert_eq!(count(&undone.commands, |c| matches!(c, EngineCommand::UnmonitorInput { .. })), 1);
        assert_eq!(count(&redone.commands, |c| matches!(c, EngineCommand::MonitorInput { .. })), 1);
    }

    #[test]
    fn test_disconnect_round_trips() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let (undone, _) = round_trip(&mut rig, |rig| rig.disconnect(osc, "Out", filter, "In"));
        assert_eq!(undone.label, "Disconnect Oscillator → SVF Filter");
        assert_eq!(count(&undone.commands, |c| matches!(c, EngineCommand::Connect { .. })), 1);
    }

    #[test]
    fn test_repatch_is_one_step() {
        let mut rig = Rig::new();
        let (osc, filter, out) = voice(&mut rig);
        // Dropping a cable on an input that already had one
        let (undone, _) = round_trip(&mut rig, |rig| rig.connect(osc, "Out", out, "Left"));
        assert_eq!(undone.label, "Repatch Oscillator → Audio Output");

        // Dragging a cable's end from one input to another
        let (undone, _) = round_trip(&mut rig, |rig| {
            rig.disconnect(osc, "Out", filter, "In");
            rig.record_held();
            rig.connect(osc, "Out", filter, "Cutoff");
        });
        assert_eq!(undone.label, "Repatch Oscillator → SVF Filter");
    }

    #[test]
    fn test_move_round_trips() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let (undone, _) = round_trip(&mut rig, |rig| rig.drag(osc, vec2(30.0, -12.0)));
        assert_eq!(undone.label, "Move Oscillator");
        assert!(undone.commands.is_empty());

        // A selection dragged together
        let (undone, _) = round_trip(&mut rig, |rig| {
            rig.drag(osc, vec2(5.0, 5.0));
            rig.drag(filter, vec2(5.0, 5.0));
        });
        assert_eq!(undone.label, "Move 2 modules");
    }

    #[test]
    fn test_move_undoes_to_the_same_place_after_zooming() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        rig.editor.pan_zoom.pan = vec2(-80.0, 35.0);
        rig.drag(osc, vec2(200.0, 150.0));
        rig.record();

        // Zooming isn't an edit
        rig.zoom(1.7);
        rig.editor.pan_zoom.pan = vec2(20.0, -60.0);
        rig.zoom(0.8);
        rig.record();
        assert_eq!(rig.history.undo_label(), Some("Move Oscillator"));

        rig.undo();
        // Back beside the filter, at this zoom
        let gap = rig.editor.node_positions[filter] - rig.editor.node_positions[osc];
        let zoom = rig.editor.pan_zoom.zoom;
        assert!((gap - vec2(300.0, 0.0) * zoom).length() < 0.01, "{gap:?}");
    }

    #[test]
    fn test_param_round_trips() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        let key = rig.engine_id(filter);
        let (undone, redone) = round_trip(&mut rig, |rig| rig.set(filter, "Cutoff", 300.0));
        assert_eq!(undone.label, "Set SVF Filter Cutoff");
        assert!(matches!(undone.commands[..],
            [EngineCommand::SetParameter { node_id, param_index: 0, value }] if node_id == key && value == 1000.0));
        assert!(matches!(redone.commands[..], [EngineCommand::SetParameter { value, .. }] if value == 300.0));
    }

    #[test]
    fn test_bypass_round_trips() {
        let mut rig = Rig::new();
        let delay = rig.add("fx.delay", pos2(0.0, 0.0));
        rig.record();
        let (undone, redone) = round_trip(&mut rig, |rig| rig.editor.graph[delay].user_data.bypassed = true);
        assert_eq!(undone.label, "Bypass Stereo Delay");
        assert!(matches!(undone.commands[..], [EngineCommand::SetBypass { bypassed: false, .. }]));
        assert!(matches!(redone.commands[..], [EngineCommand::SetBypass { bypassed: true, .. }]));
    }

    #[test]
    fn test_loading_a_sample_round_trips() {
        let mut rig = Rig::new();
        let sampler = rig.add("source.sampler", pos2(0.0, 0.0));
        rig.record();
        let (undone, _) = round_trip(&mut rig, |rig| rig.editor.graph[sampler].user_data.file = Some("C:/kit/kick.wav".into()));
        assert_eq!(undone.label, "Load sample into Sampler");
        // The app sends the recording when it sees the file change
        assert!(undone.commands.is_empty());
        rig.undo();
        assert_eq!(rig.editor.graph[sampler].user_data.file, None);
    }

    #[test]
    fn test_naming_a_section_round_trips() {
        let mut rig = Rig::new();
        let arranger = rig.add("seq.arranger", pos2(0.0, 0.0));
        rig.record();
        let (undone, _) = round_trip(&mut rig, |rig| {
            rig.editor.graph[arranger].user_data.labels.insert("Section 2".into(), "Chorus".into());
        });
        assert_eq!(undone.label, "Rename in Arranger");
        assert!(undone.commands.is_empty(), "names don't reach the engine");
        rig.undo();
        assert!(rig.editor.graph[arranger].user_data.labels.is_empty());
        rig.redo();
        assert_eq!(rig.editor.graph[arranger].user_data.labels.get("Section 2").map(String::as_str), Some("Chorus"));
    }

    #[test]
    fn test_a_deleted_arranger_comes_back_with_its_names() {
        let mut rig = Rig::new();
        let arranger = rig.add("seq.arranger", pos2(0.0, 0.0));
        rig.editor.graph[arranger].user_data.labels.insert("Lane 1".into(), "Bass".into());
        rig.record();
        let key = rig.engine_id(arranger);
        round_trip(&mut rig, |rig| rig.delete(arranger));
        rig.undo();
        assert_eq!(rig.editor.graph[rig.node(key)].user_data.labels.get("Lane 1").map(String::as_str), Some("Bass"));
    }

    #[test]
    fn test_a_follower_follows_in_the_leaders_undo_step() {
        use crate::graph::arranger_follow;
        let mut rig = Rig::new();
        let desk = rig.add("seq.arranger", pos2(0.0, 0.0));
        let cues = rig.add("seq.arranger", pos2(700.0, 0.0));
        // A frame as the app has it: followers follow, then the step's taken
        let frame = |rig: &mut Rig| {
            let leaders = arranger_follow::leaders(&rig.editor.graph);
            arranger_follow::sync(&mut rig.editor.graph, &leaders);
            rig.record();
        };
        rig.set(desk, "Length 01", 8.0);
        rig.editor.graph[desk].user_data.labels.insert("Section 1".into(), "Intro".into());
        frame(&mut rig);
        let section = |rig: &Rig, node| (rig.param(node, "Length 01"), rig.editor.graph[node].user_data.labels.get("Section 1").cloned());

        // Patching Follow brings the sections with it, and undo takes both
        rig.connect(desk, "Section", cues, "Follow");
        frame(&mut rig);
        assert_eq!(section(&rig, cues), (8.0, Some("Intro".into())));
        rig.undo();
        assert_eq!(section(&rig, cues), (4.0, None));
        rig.redo();
        assert_eq!(section(&rig, cues), (8.0, Some("Intro".into())));

        // An edit to the leader is on the follower too, in one step
        rig.set(desk, "Length 01", 2.0);
        frame(&mut rig);
        assert_eq!(rig.param(cues, "Length 01"), 2.0);
        rig.undo();
        assert_eq!((rig.param(desk, "Length 01"), rig.param(cues, "Length 01")), (8.0, 8.0));
    }

    #[test]
    fn test_a_deleted_sampler_comes_back_with_its_file() {
        let mut rig = Rig::new();
        let sampler = rig.add("source.sampler", pos2(0.0, 0.0));
        rig.editor.graph[sampler].user_data.file = Some("C:/kit/kick.wav".into());
        rig.record();
        let key = rig.engine_id(sampler);
        round_trip(&mut rig, |rig| rig.delete(sampler));
        rig.undo();
        assert_eq!(rig.editor.graph[rig.node(key)].user_data.file.as_deref(), Some("C:/kit/kick.wav"));
    }

    #[test]
    fn test_knob_drag_is_one_step() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        let steps = rig.history.undo.len();
        for cutoff in [900.0, 700.0, 500.0, 350.0] {
            rig.set(filter, "Cutoff", cutoff);
            rig.record_held();
        }
        rig.record();
        assert_eq!(rig.history.undo.len(), steps + 1);

        rig.undo();
        assert_eq!(rig.param(filter, "Cutoff"), 1000.0);
    }

    #[test]
    fn test_quick_changes_to_one_knob_merge() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        let steps = rig.history.undo.len();
        for cutoff in [900.0, 800.0, 700.0] {
            rig.set(filter, "Cutoff", cutoff);
            rig.now += Duration::from_millis(200);
            rig.history.record(&rig.editor, &rig.user_state, false, rig.now);
        }
        assert_eq!(rig.history.undo.len(), steps + 1);

        // Another knob is another step
        rig.set(filter, "Resonance", 0.9);
        rig.now += Duration::from_millis(200);
        rig.history.record(&rig.editor, &rig.user_state, false, rig.now);
        assert_eq!(rig.history.undo.len(), steps + 2);

        rig.undo();
        rig.undo();
        assert_eq!(rig.param(filter, "Cutoff"), 1000.0);
    }

    #[test]
    fn test_midi_cc_is_not_an_edit() {
        let mut rig = Rig::new();
        let (_, filter, _) = voice(&mut rig);
        let steps = rig.history.undo.len();
        rig.set(filter, "Cutoff", 2000.0);
        rig.history.absorb_param(rig.engine_id(filter), 0, 2000.0);
        rig.record();
        assert_eq!(rig.history.undo.len(), steps);
    }

    #[test]
    fn test_new_edit_clears_redo() {
        let mut rig = Rig::new();
        let (osc, _, _) = voice(&mut rig);
        rig.drag(osc, vec2(10.0, 0.0));
        rig.record();
        rig.undo();
        assert!(rig.history.redo_label().is_some());
        rig.drag(osc, vec2(0.0, 10.0));
        rig.record();
        assert!(rig.history.redo_label().is_none());
    }

    #[test]
    fn test_undo_walks_back_to_an_empty_patch() {
        let mut rig = Rig::new();
        let empty = rig.snapshot();
        let (osc, filter, _) = voice(&mut rig);
        rig.set(filter, "Cutoff", 250.0);
        rig.record();
        rig.drag(osc, vec2(0.0, 40.0));
        rig.record();
        rig.delete(filter);
        rig.record();
        let full = rig.snapshot();

        while rig.history.undo(&mut rig.editor, &mut rig.user_state).is_some() {}
        assert!(same(&rig.snapshot(), &empty));
        assert!(rig.editor.graph.nodes.is_empty());
        assert!(rig.editor.node_order.is_empty());

        while rig.history.redo(&mut rig.editor, &mut rig.user_state).is_some() {}
        assert!(same(&rig.snapshot(), &full));
    }

    fn add_frame(rig: &mut Rig, title: &str, rect: egui::Rect) -> AnnotationId {
        use crate::graph::annotations::{Frame, Tint};
        rig.user_state.annotations.add(Annotation::Frame(Frame { title: title.into(), rect, tint: Tint::Blue }))
    }

    fn frame(rig: &Rig, id: AnnotationId) -> crate::graph::annotations::Frame {
        match rig.user_state.annotations.get(id) {
            Some(Annotation::Frame(frame)) => frame.clone(),
            other => panic!("expected a frame, found {other:?}"),
        }
    }

    fn frame_mut(rig: &mut Rig, id: AnnotationId) -> &mut crate::graph::annotations::Frame {
        match rig.user_state.annotations.get_mut(id) {
            Some(Annotation::Frame(frame)) => frame,
            _ => panic!("expected a frame"),
        }
    }

    #[test]
    fn test_frames_added_moved_resized_and_deleted_undo() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let rect = egui::Rect::from_min_size(pos2(60.0, 40.0), vec2(600.0, 300.0));

        let (undone, _) = round_trip(&mut rig, |rig| {
            add_frame(rig, "Voice", rect);
        });
        assert_eq!(undone.label, "Add frame Voice");
        assert!(undone.commands.is_empty(), "frames make no sound");
        let id = rig.user_state.annotations.iter().next().unwrap().0;

        // Dragged by its title, with the modules inside it: one step
        let (undone, _) = round_trip(&mut rig, |rig| {
            frame_mut(rig, id).rect = rect.translate(vec2(40.0, 25.0));
            rig.drag(osc, vec2(40.0, 25.0));
            rig.drag(filter, vec2(40.0, 25.0));
        });
        assert_eq!(undone.label, "Move frame Voice");
        rig.undo();
        assert_eq!(frame(&rig, id).rect, rect);
        assert_eq!(rig.editor.node_positions[osc], pos2(100.0, 100.0));
        rig.redo();

        let (undone, _) = round_trip(&mut rig, |rig| frame_mut(rig, id).rect.max += vec2(80.0, 0.0));
        assert_eq!(undone.label, "Resize frame Voice");
        let (undone, _) = round_trip(&mut rig, |rig| frame_mut(rig, id).title = "Lead".into());
        assert_eq!(undone.label, "Rename frame Lead");

        // Deleted and brought back under its own ID, where it was
        let before = frame(&rig, id);
        let (undone, _) = round_trip(&mut rig, |rig| {
            rig.user_state.annotations.remove(id);
        });
        assert_eq!(undone.label, "Delete frame Lead");
        rig.undo();
        assert_eq!(frame(&rig, id), before);
    }

    #[test]
    fn test_notes_undo_and_zooming_leaves_frames_alone() {
        use crate::graph::annotations::Note;
        let mut rig = Rig::new();
        let note = rig.user_state.annotations.add(Annotation::Note(Note {
            text: "Hold a chord".into(),
            position: pos2(10.0, 10.0),
            width: 240.0,
        }));
        rig.record();
        let edit_text = |rig: &mut Rig, text: &str| {
            if let Some(Annotation::Note(n)) = rig.user_state.annotations.get_mut(note) {
                n.text = text.into();
            }
        };
        let (undone, _) = round_trip(&mut rig, |rig| edit_text(rig, "Hold a **long** chord"));
        assert_eq!(undone.label, "Edit note");

        // Frames and notes are kept in patch space, so zooming isn't an edit
        add_frame(&mut rig, "Voice", egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0)));
        rig.record();
        rig.history.mark_saved();
        rig.zoom(1.6);
        rig.record();
        assert!(!rig.history.has_unsaved_changes());
    }

    #[test]
    fn test_duplicate_is_one_named_step() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        rig.record();
        let before = rig.snapshot();

        let pasted = editing::duplicate(&mut rig.editor, &mut rig.user_state, &editing::Selection::modules(&[osc, filter])).unwrap();
        rig.history.name_next("Duplicate 2 modules");
        rig.record();
        assert_eq!(rig.history.undo_label(), Some("Duplicate 2 modules"));

        // A knob turned straight after isn't folded into the duplicate
        rig.set(pasted.nodes[1], "Cutoff", 300.0);
        rig.record();
        assert_eq!(rig.history.undo_label(), Some("Set SVF Filter Cutoff"));

        rig.undo();
        let undone = rig.undo();
        assert_eq!(undone.label, "Duplicate 2 modules");
        assert!(same(&rig.snapshot(), &before));
        assert!(pasted.nodes.iter().all(|id| !rig.editor.graph.nodes.contains_key(*id)));
    }

    /// Groups nodes the way Ctrl+G does, naming the step.
    fn group(rig: &mut Rig, nodes: &[NodeId], name: &str) -> groups::NewGroup {
        let id = rig.user_state.allocate_group_id();
        let bounds = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0));
        let new = groups::group(&mut rig.editor, nodes, id, name, bounds).unwrap();
        rig.history.name_next(format!("Group {name}"));
        new
    }

    /// Commands that change what the engine plays.
    fn sound_changes(commands: &[EngineCommand]) -> usize {
        count(commands, |c| !matches!(c,
            EngineCommand::MonitorInput { .. } | EngineCommand::UnmonitorInput { .. }
                | EngineCommand::MonitorOutput { .. } | EngineCommand::UnmonitorOutput { .. }))
    }

    #[test]
    fn test_group_and_ungroup_undo_without_touching_the_sound() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let mut new = None;
        let (undone, redone) = round_trip(&mut rig, |rig| new = Some(group(rig, &[osc, filter], "Tone")));
        assert_eq!(undone.label, "Group Tone");
        assert_eq!(sound_changes(&undone.commands), 0, "{:?}", undone.commands);
        assert_eq!(sound_changes(&redone.commands), 0, "{:?}", redone.commands);
        // Undone and redone, the group is back with its own ID
        let group_node = rig.editor.graph.nodes.iter().find(|(_, n)| n.user_data.kind.group().is_some()).unwrap().0;
        assert_eq!(rig.editor.graph[group_node].label, "Tone");
        assert_eq!(rig.editor.graph[osc].user_data.parent, rig.editor.graph[group_node].user_data.kind.group());
        let _ = new;

        let (undone, _) = round_trip(&mut rig, |rig| {
            groups::ungroup(&mut rig.editor, group_node).unwrap();
            rig.history.name_next("Ungroup Tone");
        });
        assert_eq!(undone.label, "Ungroup Tone");
        assert_eq!(sound_changes(&undone.commands), 0, "{:?}", undone.commands);
    }

    #[test]
    fn test_renaming_and_pinning_undo() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let new = group(&mut rig, &[osc, filter], "Group");
        rig.record();

        let (undone, _) = round_trip(&mut rig, |rig| groups::rename(&mut rig.editor.graph, new.node, "Tone"));
        assert_eq!(undone.label, "Rename group Tone");
        rig.undo();
        assert_eq!(rig.editor.graph[new.node].label, "Group");
        rig.redo();

        let (undone, _) = round_trip(&mut rig, |rig| {
            rig.editor.graph[filter].user_data.pins.insert("Cutoff".into(), 1);
        });
        assert_eq!(undone.label, "Pin SVF Filter knob");
        assert!(undone.commands.is_empty());
        rig.undo();
        assert!(rig.editor.graph[filter].user_data.pins.is_empty());
        rig.redo();

        // Naming it on the face: no sound changes, and undo takes the name off
        let (undone, redone) = round_trip(&mut rig, |rig| {
            rig.editor.graph[filter].user_data.face_labels.insert("Cutoff".into(), "Bright".into());
        });
        assert_eq!(undone.label, "Name SVF Filter control on face");
        assert!(undone.commands.is_empty() && redone.commands.is_empty());
        assert_eq!(rig.editor.graph[filter].user_data.face_label("Cutoff"), Some("Bright"));
        rig.undo();
        assert_eq!(rig.editor.graph[filter].user_data.face_label("Cutoff"), None);
        assert_eq!(rig.editor.graph[filter].user_data.pin_levels("Cutoff"), 1);
    }

    #[test]
    fn test_jacks_shown_and_removed_undo_with_their_cables() {
        let mut rig = Rig::new();
        let (osc, filter, out) = voice(&mut rig);
        let lfo = rig.add("mod.lfo", pos2(100.0, 400.0));
        let new = group(&mut rig, &[osc, filter], "Tone");
        rig.record();
        let jacks = |rig: &Rig| groups::input_jacks(&rig.editor.graph, new.node).into_iter().map(|j| j.name).collect::<Vec<_>>();
        let leaf = |rig: &Rig| groups::leaf_cables(&rig.editor.graph).len();
        assert!(jacks(&rig).is_empty());

        // A new jack for the filter's Cutoff, with the LFO plugged into it
        let (undone, _) = round_trip(&mut rig, |rig| {
            groups::show_port(&mut rig.editor.graph, filter, Side::Input, "Cutoff").unwrap();
            rig.history.name_next("Add jack Cutoff to Tone");
        });
        assert_eq!(undone.label, "Add jack Cutoff to Tone");
        assert_eq!(jacks(&rig), ["Cutoff"]);
        rig.connect(lfo, "Out", new.node, "Cutoff");
        rig.record();
        let cables = leaf(&rig);

        // Taking it away undoes with the LFO still plugged in outside
        round_trip(&mut rig, |rig| {
            groups::remove_port_jack(&mut rig.editor.graph, new.inputs, Side::Output, "Cutoff").unwrap();
        });
        assert!(jacks(&rig).is_empty());
        assert_eq!(leaf(&rig), cables - 1);
        rig.undo();
        assert_eq!(jacks(&rig), ["Cutoff"]);
        assert_eq!(leaf(&rig), cables);

        // An output jack beside the one grouping made, hidden from inside
        let outputs = |rig: &Rig| groups::output_jacks(&rig.editor.graph, new.node).into_iter().map(|j| j.name).collect::<Vec<_>>();
        round_trip(&mut rig, |rig| {
            groups::show_port(&mut rig.editor.graph, filter, Side::Output, "HighPass").unwrap();
        });
        assert_eq!(outputs(&rig), ["LowPass", "HighPass"]);
        rig.connect(new.node, "HighPass", out, "Right");
        rig.record();
        let cables = leaf(&rig);
        round_trip(&mut rig, |rig| {
            groups::hide_port(&mut rig.editor.graph, filter, Side::Output, "HighPass").unwrap();
        });
        assert_eq!(outputs(&rig), ["LowPass"]);
        rig.undo();
        assert_eq!(outputs(&rig), ["LowPass", "HighPass"]);
        assert_eq!(leaf(&rig), cables);

        // Removing the first output jack moves the second up a slot, and
        // undo puts both back where they were, cables and all
        round_trip(&mut rig, |rig| {
            groups::remove_port_jack(&mut rig.editor.graph, new.node, Side::Output, "LowPass").unwrap();
        });
        assert_eq!(outputs(&rig), ["HighPass"]);
        assert_eq!(leaf(&rig), cables - 1);
        let undone = rig.undo();
        assert_eq!(sound_changes(&undone.commands), 1, "{:?}", undone.commands);
        assert_eq!(outputs(&rig), ["LowPass", "HighPass"]);
        assert_eq!(leaf(&rig), cables);
    }

    #[test]
    fn test_renaming_and_moving_jacks_undo_with_their_cables() {
        let mut rig = Rig::new();
        let (osc, filter, out) = voice(&mut rig);
        let new = group(&mut rig, &[osc, filter], "Tone");
        groups::show_port(&mut rig.editor.graph, filter, Side::Output, "HighPass").unwrap();
        rig.connect(new.node, "HighPass", out, "Right");
        rig.record();
        let outputs = |rig: &Rig| groups::output_jacks(&rig.editor.graph, new.node).into_iter().map(|j| j.name).collect::<Vec<_>>();
        let cables = groups::leaf_cables(&rig.editor.graph);
        assert_eq!(outputs(&rig), ["LowPass", "HighPass"]);

        let (undone, _) = round_trip(&mut rig, |rig| {
            groups::rename_jack(&mut rig.editor.graph, new.outputs, Side::Input, "HighPass", "Bright").unwrap();
            rig.history.name_next("Rename jack HighPass to Bright");
        });
        assert_eq!(undone.label, "Rename jack HighPass to Bright");
        assert!(undone.commands.is_empty(), "{:?}", undone.commands);
        assert_eq!(outputs(&rig), ["LowPass", "Bright"]);

        // Moving swaps the jacks, and every module still hears what it did
        let (undone, redone) = round_trip(&mut rig, |rig| {
            groups::move_jack(&mut rig.editor.graph, new.node, Side::Output, "Bright", true).unwrap();
        });
        assert_eq!(sound_changes(&undone.commands), 0, "{:?}", undone.commands);
        assert_eq!(sound_changes(&redone.commands), 0, "{:?}", redone.commands);
        assert_eq!(outputs(&rig), ["Bright", "LowPass"]);
        assert_eq!(groups::leaf_cables(&rig.editor.graph), cables);
        rig.undo();
        assert_eq!(outputs(&rig), ["LowPass", "Bright"]);
        assert_eq!(groups::leaf_cables(&rig.editor.graph), cables);
    }

    #[test]
    fn test_deleting_a_group_brings_everything_back() {
        let mut rig = Rig::new();
        let (osc, filter, out) = voice(&mut rig);
        rig.set(filter, "Cutoff", 520.0);
        let new = group(&mut rig, &[osc, filter], "Tone");
        rig.record();
        let keys = [rig.engine_id(osc), rig.engine_id(filter)];

        let (undone, redone) = round_trip(&mut rig, |rig| {
            let commands = editing::delete_modules(&mut rig.editor, &mut rig.user_state, &[new.node]);
            assert_eq!(count(&commands, |c| matches!(c, EngineCommand::RemoveModule { .. })), 2);
        });
        // Both modules come back under their engine IDs, with the cable out
        // of the group to the output, through its jacks
        for key in keys {
            assert_eq!(count(&undone.commands, |c| matches!(c, EngineCommand::AddModule { node_id, .. } if *node_id == key)), 1);
        }
        assert_eq!(count(&undone.commands, |c| matches!(c, EngineCommand::Connect { .. })), 2);
        assert_eq!(count(&redone.commands, |c| matches!(c, EngineCommand::RemoveModule { .. })), 2);
        rig.undo();
        assert_eq!(rig.param(rig.node(keys[1]), "Cutoff"), 520.0);
        assert!(rig.editor.graph.nodes.contains_key(rig.node(keys[0])));
        let _ = out;
    }

    #[test]
    fn test_an_edit_inside_a_group_remembers_where_it_was_made() {
        let mut rig = Rig::new();
        let (osc, filter, _) = voice(&mut rig);
        let new = group(&mut rig, &[osc, filter], "Tone");
        rig.record();
        let id = rig.editor.graph[new.node].user_data.kind.group();

        rig.user_state.level = id;
        rig.set(filter, "Cutoff", 300.0);
        rig.record();
        rig.user_state.level = None;
        let undone = rig.undo();
        assert_eq!(undone.label, "Set SVF Filter Cutoff");
        assert_eq!(undone.level, id);
        let redone = rig.redo();
        assert_eq!(redone.level, id);
    }

    #[test]
    fn test_a_saved_loop_file_is_not_an_edit_and_undo_keeps_it() {
        let mut rig = Rig::new();
        let looper = rig.add("util.looper", pos2(0.0, 0.0));
        rig.record();
        let key = rig.engine_id(looper);
        rig.set(looper, "Feedback", 0.5);
        rig.record();
        let steps = rig.history.undo.len();

        // A save points the Looper at its loop's file
        rig.editor.graph[looper].user_data.file = Some("C:/song loops/Looper 1.wav".into());
        rig.history.absorb_file(key, Some("C:/song loops/Looper 1.wav".into()));
        rig.record();
        assert_eq!(rig.history.undo.len(), steps, "no step");

        // Undoing the knob, or the Looper's arrival and back, keeps the file
        rig.undo();
        assert_eq!(rig.param(looper, "Feedback"), 1.0);
        assert_eq!(rig.editor.graph[looper].user_data.file.as_deref(), Some("C:/song loops/Looper 1.wav"));
        rig.undo();
        rig.redo();
        assert_eq!(rig.editor.graph[rig.node(key)].user_data.file.as_deref(), Some("C:/song loops/Looper 1.wav"));
    }
}
