//! Groups: modules collapsed into one node, with jacks of its own and
//! whichever controls are pinned to its face.
//!
//! # How a group lives in the graph
//!
//! Every module stays in the editor graph whether or not it's in a group,
//! and the editor shows one level at a time. A group is three nodes:
//!
//! - the **group node**, on the level the group sits on, whose inputs and
//!   outputs are the group's jacks;
//! - **Inputs**, inside the group, with an output for each input jack;
//! - **Outputs**, inside the group, with an input for each output jack.
//!
//! A cable into a group's input jack carries on, inside, from the same jack
//! on Inputs; one into a jack on Outputs carries on out of the group's output
//! jack. [`leaf_source`] follows those hops, so [`leaf_cables`] are the
//! module-to-module cables the patch adds up to. That's all the engine ever
//! hears of: groups cost nothing to run, and grouping or ungrouping doesn't
//! change a sample.

use std::collections::{HashMap, HashSet};

use egui::{Color32, Pos2, Rect, Vec2};
use egui_node_graph2::{AnyParameterId, InputId, InputParamKind, NodeId, OutputId};

use crate::app::theme;
use crate::dsp::{ModuleCategory, SignalType};
use super::{SynthDataType, SynthGraph, SynthGraphEditorState, SynthNodeData, SynthValueType};

/// The `module_id` the three nodes of a group carry. No module has these.
pub const GROUP_ID: &str = "group";
pub const INPUTS_ID: &str = "group.inputs";
pub const OUTPUTS_ID: &str = "group.outputs";

/// Most groups inside groups that are followed, in case a bad patch makes
/// a loop of them.
const MAX_DEPTH: usize = 64;

/// How big a module is taken to be where its real size isn't known (it's
/// hidden inside a group), in unzoomed points.
pub const NOMINAL_NODE_SIZE: Vec2 = Vec2::new(180.0, 120.0);

/// Space between a group's modules and its Inputs and Outputs, unzoomed.
const PROXY_GAP: f32 = 70.0;
/// Room for the Inputs node, left of the modules, unzoomed.
const INPUTS_WIDTH: f32 = 110.0;

/// Identifies a group. Taken from the same counter as engine node IDs, so a
/// patch file can use both as node IDs without them colliding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);

/// What a node in the editor graph is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeKind {
    /// A module, which the engine runs.
    #[default]
    Module,
    /// A group seen from outside.
    Group(GroupId),
    /// Inside a group: where its input jacks come in.
    Inputs(GroupId),
    /// Inside a group: where its output jacks go out.
    Outputs(GroupId),
}

impl NodeKind {
    /// The group this is the outside of.
    pub fn group(self) -> Option<GroupId> {
        match self {
            Self::Group(id) => Some(id),
            _ => None,
        }
    }

    /// Inputs or Outputs, which exist only as part of their group.
    pub fn is_proxy(self) -> bool {
        matches!(self, Self::Inputs(_) | Self::Outputs(_))
    }
}

/// One of a group's jacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jack {
    pub name: String,
    pub signal: SignalType,
}

/// The nodes that make up one group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub node: Option<NodeId>,
    pub inputs: Option<NodeId>,
    pub outputs: Option<NodeId>,
}

/// Where each group's nodes are in a graph.
#[derive(Clone, Debug, Default)]
pub struct GroupIndex {
    parts: HashMap<GroupId, Parts>,
}

impl GroupIndex {
    pub fn of(graph: &SynthGraph) -> Self {
        let mut parts: HashMap<GroupId, Parts> = HashMap::new();
        for (node_id, node) in graph.nodes.iter() {
            match node.user_data.kind {
                NodeKind::Module => {}
                NodeKind::Group(id) => parts.entry(id).or_default().node = Some(node_id),
                NodeKind::Inputs(id) => parts.entry(id).or_default().inputs = Some(node_id),
                NodeKind::Outputs(id) => parts.entry(id).or_default().outputs = Some(node_id),
            }
        }
        Self { parts }
    }

    pub fn parts(&self, id: GroupId) -> Parts {
        self.parts.get(&id).copied().unwrap_or_default()
    }

    /// The group's own node, if the group exists.
    pub fn node(&self, id: GroupId) -> Option<NodeId> {
        self.parts(id).node
    }

    /// Every group in the graph.
    pub fn ids(&self) -> impl Iterator<Item = GroupId> + '_ {
        self.parts.iter().filter(|(_, p)| p.node.is_some()).map(|(id, _)| *id)
    }

    /// The group a group sits in, or `None` if it's at the top.
    pub fn parent_of(&self, graph: &SynthGraph, id: GroupId) -> Option<GroupId> {
        graph.nodes.get(self.node(id)?)?.user_data.parent
    }

    /// The groups from the top of the patch down to `level`, outermost
    /// first. Empty at the top. A level that no longer exists has no path.
    pub fn path(&self, graph: &SynthGraph, level: Option<GroupId>) -> Option<Vec<GroupId>> {
        let mut path = Vec::new();
        let mut at = level;
        while let Some(id) = at {
            self.node(id)?;
            path.push(id);
            if path.len() > MAX_DEPTH {
                return None;
            }
            at = self.parent_of(graph, id);
        }
        path.reverse();
        Some(path)
    }

    /// How many groups down from `outer` a node sitting in `level` is: 1
    /// directly inside it, 2 inside a group inside it, and so on. `None` if
    /// it isn't inside `outer` at all.
    pub fn depth_below(&self, graph: &SynthGraph, outer: GroupId, level: Option<GroupId>) -> Option<u8> {
        let mut at = level?;
        for depth in 1..=MAX_DEPTH as u8 {
            if at == outer {
                return Some(depth);
            }
            at = self.parent_of(graph, at)?;
        }
        None
    }
}

/// The module output a cable from `output` really carries, through any
/// number of group jacks. `None` if the trail ends at a jack with nothing
/// plugged into it.
pub fn leaf_source(graph: &SynthGraph, index: &GroupIndex, mut output: OutputId) -> Option<OutputId> {
    for _ in 0..MAX_DEPTH {
        let node = graph.nodes.get(graph.outputs.get(output)?.node)?;
        let position = node.outputs.iter().position(|(_, id)| *id == output)?;
        let jack = match node.user_data.kind {
            NodeKind::Module => return Some(output),
            // Inside a group, a jack carries what's plugged into the group outside
            NodeKind::Inputs(id) => graph.nodes.get(index.node(id)?)?.inputs.get(position)?.1,
            // Outside, a group's output carries what's plugged into it inside
            NodeKind::Group(id) => graph.nodes.get(index.parts(id).outputs?)?.inputs.get(position)?.1,
            NodeKind::Outputs(_) => return None,
        };
        output = graph.connections.get(jack)?.first().copied()?;
    }
    None
}

/// Every cable from one module's output to another module's input, through
/// any groups between them: the patch as the engine hears it.
pub fn leaf_cables(graph: &SynthGraph) -> Vec<(OutputId, InputId)> {
    let index = GroupIndex::of(graph);
    graph
        .iter_connections()
        .filter(|(input, _)| {
            graph.inputs.get(*input).and_then(|i| graph.nodes.get(i.node)).is_some_and(|n| n.user_data.is_module())
        })
        .filter_map(|(input, output)| Some((leaf_source(graph, &index, output)?, input)))
        .collect()
}

/// Gives groups new IDs, wherever the graph names them: as groups, and as
/// where nodes sit. IDs `new` doesn't mention stay as they are.
pub fn renumber(graph: &mut SynthGraph, new: &HashMap<GroupId, GroupId>) {
    let renumbered = |id: GroupId| new.get(&id).copied().unwrap_or(id);
    for node in graph.nodes.values_mut() {
        let data = &mut node.user_data;
        data.kind = match data.kind {
            NodeKind::Module => NodeKind::Module,
            NodeKind::Group(id) => NodeKind::Group(renumbered(id)),
            NodeKind::Inputs(id) => NodeKind::Inputs(renumbered(id)),
            NodeKind::Outputs(id) => NodeKind::Outputs(renumbered(id)),
        };
        data.parent = data.parent.map(renumbered);
    }
}

/// The nodes on one level: at the top of the patch, or directly inside a group.
pub fn level_nodes(graph: &SynthGraph, level: Option<GroupId>) -> Vec<NodeId> {
    graph.nodes.iter().filter(|(_, n)| n.user_data.parent == level).map(|(id, _)| id).collect()
}

/// Every node inside a group, however deep, including its Inputs and
/// Outputs and the nodes of groups inside it, but not the group's own node.
pub fn descendants(graph: &SynthGraph, index: &GroupIndex, id: GroupId) -> Vec<NodeId> {
    graph
        .nodes
        .iter()
        .filter(|(_, node)| index.depth_below(graph, id, node.user_data.parent).is_some())
        .map(|(node_id, _)| node_id)
        .collect()
}

/// A group's name, as its node shows it.
pub fn name(graph: &SynthGraph, node_id: NodeId) -> &str {
    graph.nodes.get(node_id).map_or("", |n| n.user_data.display_name.as_str())
}

/// The group a capture's `enter` cue names: the group of that name on the
/// level `level`, or else the only one of that name anywhere. A path of
/// names from the top of the patch, `Bells/FM Bell`, picks one out where
/// several share a name, and `Pump#2` the second of several side by side,
/// counting as a page reads: down, then across. `position` is where a node
/// sits.
pub fn find_group(
    graph: &SynthGraph,
    position: impl Fn(NodeId) -> Option<Pos2>,
    level: Option<GroupId>,
    wanted: &str,
) -> Result<GroupId, String> {
    let index = GroupIndex::of(graph);
    // The groups in `parent` (anywhere, for `None`) called `wanted`, as a page reads
    let named = |wanted: &str, parent: Option<Option<GroupId>>| -> Vec<GroupId> {
        let mut found: Vec<(GroupId, Pos2)> = index
            .ids()
            .filter(|&id| parent.is_none_or(|p| index.parent_of(graph, id) == p))
            .filter_map(|id| {
                let node = index.node(id)?;
                (name(graph, node) == wanted).then(|| (id, position(node).unwrap_or_default()))
            })
            .collect();
        found.sort_by(|a, b| a.1.y.total_cmp(&b.1.y).then(a.1.x.total_cmp(&b.1.x)));
        found.into_iter().map(|(id, _)| id).collect()
    };
    // A name, or the nth of its name on one level: `Pump#2`
    let called = |part: &str, parent: Option<Option<GroupId>>| -> Vec<GroupId> {
        let part = part.trim();
        let whole = named(part, parent);
        match part.rsplit_once('#').map(|(name, n)| (name.trim(), n.parse::<usize>())) {
            Some((name, Ok(n))) if whole.is_empty() && n >= 1 && parent.is_some() => {
                named(name, parent).get(n - 1).copied().into_iter().collect()
            }
            _ => whole,
        }
    };
    let one = |ids: Vec<GroupId>, what: &str| match ids.as_slice() {
        [id] => Ok(*id),
        [] => Err(format!("no group called {what}")),
        _ => Err(format!("{} groups called {what}: give its path, as Outer/Inner, or which, as {what}#2", ids.len())),
    };
    let anywhere = called(wanted, None);
    if anywhere.is_empty() && wanted.contains('/') {
        let mut at = None;
        for part in wanted.split('/') {
            at = Some(one(called(part, Some(at)), wanted)?);
        }
        return at.ok_or_else(|| format!("no group called {wanted}"));
    }
    match called(wanted, Some(level)).as_slice() {
        [id] => Ok(*id),
        _ => one(anywhere, wanted.trim()),
    }
}

/// Renames a group.
pub fn rename(graph: &mut SynthGraph, node_id: NodeId, name: &str) {
    if let Some(node) = graph.nodes.get_mut(node_id) {
        node.label = name.to_string();
        node.user_data.display_name = name.to_string();
    }
}

/// A node's input jacks, as jacks.
pub fn input_jacks(graph: &SynthGraph, node_id: NodeId) -> Vec<Jack> {
    graph.nodes.get(node_id).map_or_else(Vec::new, |node| {
        node.inputs
            .iter()
            .map(|(name, id)| Jack { name: name.clone(), signal: graph.get_input(*id).typ.signal_type() })
            .collect()
    })
}

/// A node's output jacks, as jacks.
pub fn output_jacks(graph: &SynthGraph, node_id: NodeId) -> Vec<Jack> {
    graph.nodes.get(node_id).map_or_else(Vec::new, |node| {
        node.outputs
            .iter()
            .map(|(name, id)| Jack { name: name.clone(), signal: graph.get_output(*id).typ.signal_type() })
            .collect()
    })
}

fn node_data(module_id: &'static str, name: &str, kind: NodeKind, parent: Option<GroupId>) -> SynthNodeData {
    let mut data = SynthNodeData::new(module_id, name, ModuleCategory::Utility);
    data.kind = kind;
    data.parent = parent;
    data
}

fn add_jack_inputs(graph: &mut SynthGraph, node_id: NodeId, jacks: &[Jack]) {
    for jack in jacks {
        graph.add_input_param(
            node_id,
            jack.name.clone(),
            SynthDataType::new(jack.signal),
            SynthValueType::Port,
            InputParamKind::ConnectionOnly,
            true,
        );
    }
}

fn add_jack_outputs(graph: &mut SynthGraph, node_id: NodeId, jacks: &[Jack]) {
    for jack in jacks {
        graph.add_output_param(node_id, jack.name.clone(), SynthDataType::new(jack.signal));
    }
}

/// Adds a group's own node, on the level `parent`.
pub fn add_group_node(
    graph: &mut SynthGraph,
    id: GroupId,
    name: &str,
    parent: Option<GroupId>,
    inputs: &[Jack],
    outputs: &[Jack],
) -> NodeId {
    let data = node_data(GROUP_ID, name, NodeKind::Group(id), parent)
        .with_description("Modules grouped into one. Double-click to open it (or Tab)");
    graph.add_node(name.to_string(), data, |graph, node_id| {
        add_jack_inputs(graph, node_id, inputs);
        add_jack_outputs(graph, node_id, outputs);
    })
}

/// Adds the Inputs node inside a group, with an output for each input jack.
pub fn add_inputs_node(graph: &mut SynthGraph, id: GroupId, jacks: &[Jack]) -> NodeId {
    let data = node_data(INPUTS_ID, "Inputs", NodeKind::Inputs(id), Some(id))
        .with_description("What's plugged into the group's input jacks, outside");
    graph.add_node("Inputs".to_string(), data, |graph, node_id| add_jack_outputs(graph, node_id, jacks))
}

/// Adds the Outputs node inside a group, with an input for each output jack.
pub fn add_outputs_node(graph: &mut SynthGraph, id: GroupId, jacks: &[Jack]) -> NodeId {
    let data = node_data(OUTPUTS_ID, "Outputs", NodeKind::Outputs(id), Some(id))
        .with_description("What the group's output jacks carry, outside");
    graph.add_node("Outputs".to_string(), data, |graph, node_id| add_jack_inputs(graph, node_id, jacks))
}

/// Puts a node on the canvas, on top of the others.
fn place(editor: &mut SynthGraphEditorState, node_id: NodeId, position: Pos2) {
    editor.node_positions.insert(node_id, position);
    editor.node_order.push(node_id);
}

/// Names made unique by numbering repeats: "In", "In 2", "In 3".
fn unique_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    names
        .into_iter()
        .map(|name| {
            let mut candidate = name.clone();
            let mut n = 2;
            while !seen.insert(candidate.clone()) {
                candidate = format!("{name} {n}");
                n += 1;
            }
            candidate
        })
        .collect()
}

/// The three nodes a new group is made of.
#[derive(Clone, Copy, Debug)]
pub struct NewGroup {
    pub node: NodeId,
    pub inputs: NodeId,
    pub outputs: NodeId,
}

/// Collapses nodes on one level into a group called `name`.
///
/// Every cable crossing the selection's edge becomes a jack: one input jack
/// for each output outside that feeds the selection, named after the first
/// input it feeds; one output jack for each output inside that feeds
/// something outside, named after that output. Jacks are ordered top to
/// bottom by where their modules sit. The cables outside plug into the
/// group's jacks, and those inside into Inputs and Outputs, so the modules
/// still hear exactly what they did.
///
/// The group's node takes the top-left corner of the selection, and
/// `bounds` (the selection as drawn, in editor coordinates) places Inputs
/// and Outputs either side of it. Nodes not on the same level as the first
/// one, and other groups' Inputs and Outputs, are left out. Returns `None`
/// if nothing could be grouped.
pub fn group(
    editor: &mut SynthGraphEditorState,
    nodes: &[NodeId],
    id: GroupId,
    name: &str,
    bounds: Rect,
) -> Option<NewGroup> {
    let graph = &editor.graph;
    let groupable = |n: &NodeId| graph.nodes.get(*n).is_some_and(|node| !node.user_data.kind.is_proxy());
    let level = graph.nodes.get(*nodes.iter().find(|n| groupable(n))?)?.user_data.parent;
    let inside: Vec<NodeId> = nodes
        .iter()
        .copied()
        .filter(|n| groupable(n) && graph[*n].user_data.parent == level)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let is_inside = |node_id: NodeId| inside.contains(&node_id);
    let position = |node_id: NodeId| editor.node_positions.get(node_id).copied().unwrap_or_default();
    // Top to bottom, then left to right, then in port order
    let place_key = |node_id: NodeId, port: usize| {
        let p = position(node_id);
        ((p.y * 4.0) as i64, (p.x * 4.0) as i64, port)
    };

    let mut inbound = Vec::new();
    let mut outbound = Vec::new();
    for (input, output) in graph.iter_connections() {
        let (to, from) = (graph.get_input(input).node, graph.get_output(output).node);
        match (is_inside(from), is_inside(to)) {
            (false, true) => inbound.push((input, output)),
            (true, false) => outbound.push((input, output)),
            _ => {}
        }
    }
    let input_position = |input: InputId| {
        let node = graph.get_input(input).node;
        place_key(node, graph[node].inputs.iter().position(|(_, id)| *id == input).unwrap_or(0))
    };
    let output_position = |output: OutputId| {
        let node = graph.get_output(output).node;
        place_key(node, graph[node].outputs.iter().position(|(_, id)| *id == output).unwrap_or(0))
    };
    inbound.sort_by_key(|&(input, _)| input_position(input));
    outbound.sort_by_key(|&(_, output)| output_position(output));

    // One input jack per output outside, one output jack per output inside
    let mut in_sources: Vec<OutputId> = Vec::new();
    let mut in_names = Vec::new();
    let mut in_signals = Vec::new();
    for &(input, output) in &inbound {
        if !in_sources.contains(&output) {
            in_sources.push(output);
            let node = &graph[graph.get_input(input).node];
            in_names.push(node.inputs.iter().find(|(_, id)| *id == input).map_or_else(String::new, |(n, _)| n.clone()));
            in_signals.push(graph.get_input(input).typ.signal_type());
        }
    }
    let mut out_sources: Vec<OutputId> = Vec::new();
    let mut out_names = Vec::new();
    let mut out_signals = Vec::new();
    for &(_, output) in &outbound {
        if !out_sources.contains(&output) {
            out_sources.push(output);
            let node = &graph[graph.get_output(output).node];
            out_names.push(node.outputs.iter().find(|(_, id)| *id == output).map_or_else(String::new, |(n, _)| n.clone()));
            out_signals.push(graph.get_output(output).typ.signal_type());
        }
    }
    let jacks = |names: Vec<String>, signals: Vec<SignalType>| -> Vec<Jack> {
        unique_names(names).into_iter().zip(signals).map(|(name, signal)| Jack { name, signal }).collect()
    };
    let input_jacks = jacks(in_names, in_signals);
    let output_jacks = jacks(out_names, out_signals);

    let corner = inside.iter().map(|&n| position(n)).reduce(|a, b| a.min(b))?;
    let zoom = editor.pan_zoom.zoom;
    let graph = &mut editor.graph;
    let node = add_group_node(graph, id, name, level, &input_jacks, &output_jacks);
    let inputs = add_inputs_node(graph, id, &input_jacks);
    let outputs = add_outputs_node(graph, id, &output_jacks);

    // Rewire through the jacks
    for (input, output) in inbound {
        let k = in_sources.iter().position(|o| *o == output)?;
        graph.remove_connection(input, output);
        let (jack_in, jack_out) = (graph[node].inputs[k].1, graph[inputs].outputs[k].1);
        graph.add_connection(output, jack_in, 0);
        graph.add_connection(jack_out, input, 0);
    }
    for (input, output) in outbound {
        let k = out_sources.iter().position(|o| *o == output)?;
        graph.remove_connection(input, output);
        let (jack_in, jack_out) = (graph[outputs].inputs[k].1, graph[node].outputs[k].1);
        graph.add_connection(output, jack_in, 0);
        graph.add_connection(jack_out, input, 0);
    }
    for &node_id in &inside {
        graph[node_id].user_data.parent = Some(id);
    }

    let inputs_at = Pos2::new(bounds.left() - (PROXY_GAP + INPUTS_WIDTH) * zoom, bounds.top());
    let outputs_at = Pos2::new(bounds.right() + PROXY_GAP * zoom, bounds.top());
    place(editor, inputs, inputs_at);
    place(editor, outputs, outputs_at);
    place(editor, node, corner);
    Some(NewGroup { node, inputs, outputs })
}

/// Which side of a node a port is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Input,
    Output,
}

/// What a port's right-click menu can do with a group's jacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JackAction {
    /// Give the group the port sits in a jack for it.
    Show,
    /// An input fed by something else in the group can't take a jack too.
    Taken,
    /// Unplug it from the group's jack, and drop the jack if that leaves it
    /// carrying nothing.
    Hide,
    /// The port is one of a group's jacks, on its node or its Inputs or
    /// Outputs: rename it, move it, or take it away. `slot` is its place
    /// among the `count` jacks on its side.
    Jack { slot: usize, count: usize },
}

/// What a port's right-click menu chose to do with a group's jacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JackEdit {
    Show,
    Hide,
    Remove,
    Rename(String),
    /// One place up the group's jacks, or down.
    Move { up: bool },
}

/// The port called `name` on one side of a node, and its position there.
fn port(graph: &SynthGraph, node_id: NodeId, side: Side, name: &str) -> Option<(usize, AnyParameterId)> {
    let node = graph.nodes.get(node_id)?;
    match side {
        Side::Input => node.inputs.iter().position(|(n, _)| n == name).map(|k| (k, AnyParameterId::Input(node.inputs[k].1))),
        Side::Output => node.outputs.iter().position(|(n, _)| n == name).map(|k| (k, AnyParameterId::Output(node.outputs[k].1))),
    }
}

/// The group whose jacks a node's ports on one side are, and on which side
/// of the group's own node they sit: a group's node has its jacks both
/// sides, Inputs carries the input jacks and Outputs the output jacks.
fn jacks_of(kind: NodeKind, side: Side) -> Option<(GroupId, Side)> {
    match (kind, side) {
        (NodeKind::Group(id), side) => Some((id, side)),
        (NodeKind::Inputs(id), Side::Output) => Some((id, Side::Input)),
        (NodeKind::Outputs(id), Side::Input) => Some((id, Side::Output)),
        _ => None,
    }
}

/// What can be done with groups' jacks from a port, in menu order. A
/// group's node inside another group has both: its own jack can go, and it
/// can be shown on the group around it.
pub fn jack_actions(graph: &SynthGraph, index: &GroupIndex, node_id: NodeId, side: Side, name: &str) -> Vec<JackAction> {
    let mut actions = Vec::new();
    let Some(node) = graph.nodes.get(node_id) else { return actions };
    let Some((_, id)) = port(graph, node_id, side, name) else { return actions };
    let data = &node.user_data;
    if jacks_of(data.kind, side).is_some() {
        let ports = match side {
            Side::Input => node.inputs.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            Side::Output => node.outputs.iter().map(|(n, _)| n).collect(),
        };
        if let Some(slot) = ports.iter().position(|n| *n == name) {
            actions.push(JackAction::Jack { slot, count: ports.len() });
        }
    }
    let Some(parent) = data.parent.filter(|_| !data.kind.is_proxy()) else { return actions };
    let parts = index.parts(parent);
    match id {
        AnyParameterId::Input(input) => {
            if matches!(graph.get_input(input).kind, InputParamKind::ConstantOnly) {
                return actions;
            }
            actions.push(match graph.connection(input) {
                None => JackAction::Show,
                Some(source) if Some(graph.get_output(source).node) == parts.inputs => JackAction::Hide,
                Some(_) => JackAction::Taken,
            });
        }
        AnyParameterId::Output(output) => {
            let shown = parts.outputs.and_then(|o| graph.nodes.get(o)).is_some_and(|outputs| {
                outputs.inputs.iter().any(|(_, jack)| graph.connection(*jack) == Some(output))
            });
            actions.push(if shown { JackAction::Hide } else { JackAction::Show });
        }
    }
    actions
}

/// Gives the group a port sits in a new jack for it, at the bottom of the
/// group's jacks, named after the port and wired to it inside. Returns the
/// group and the jack's name, or `None` if the port can't have one.
pub fn show_port(graph: &mut SynthGraph, node_id: NodeId, side: Side, name: &str) -> Option<(GroupId, String)> {
    let index = GroupIndex::of(graph);
    if !jack_actions(graph, &index, node_id, side, name).contains(&JackAction::Show) {
        return None;
    }
    let id = graph.nodes.get(node_id)?.user_data.parent?;
    let parts = index.parts(id);
    let (group, (_, port)) = (parts.node?, port(graph, node_id, side, name)?);
    let signal = match port {
        AnyParameterId::Input(input) => graph.get_input(input).typ.signal_type(),
        AnyParameterId::Output(output) => graph.get_output(output).typ.signal_type(),
    };
    let taken: Vec<String> = match side {
        Side::Input => graph[group].inputs.iter().map(|(n, _)| n.clone()).collect(),
        Side::Output => graph[group].outputs.iter().map(|(n, _)| n.clone()).collect(),
    };
    let names = unique_names(taken.into_iter().chain(std::iter::once(name.to_string())));
    let jack = Jack { name: names.last()?.clone(), signal };
    match port {
        AnyParameterId::Input(input) => {
            let inputs = parts.inputs?;
            add_jack_inputs(graph, group, std::slice::from_ref(&jack));
            add_jack_outputs(graph, inputs, std::slice::from_ref(&jack));
            let carried = graph[inputs].outputs.last()?.1;
            graph.add_connection(carried, input, 0);
        }
        AnyParameterId::Output(output) => {
            let outputs = parts.outputs?;
            add_jack_outputs(graph, group, std::slice::from_ref(&jack));
            add_jack_inputs(graph, outputs, std::slice::from_ref(&jack));
            let carries = graph[outputs].inputs.last()?.1;
            graph.add_connection(output, carries, 0);
        }
    }
    Some((id, jack.name))
}

/// Unplugs a port from the jacks of the group it sits in. A jack left
/// carrying nothing inside goes, with its cables outside. Returns the group
/// and each jack it was plugged into, with whether the jack went.
pub fn hide_port(graph: &mut SynthGraph, node_id: NodeId, side: Side, name: &str) -> Option<(GroupId, Vec<(String, bool)>)> {
    let index = GroupIndex::of(graph);
    let id = graph.nodes.get(node_id)?.user_data.parent?;
    let parts = index.parts(id);
    let (_, port) = port(graph, node_id, side, name)?;
    let mut jacks = Vec::new();
    match port {
        AnyParameterId::Input(input) => {
            let source = graph.connection(input)?;
            if Some(graph.get_output(source).node) != parts.inputs {
                return None;
            }
            graph.remove_connection(input, source);
            let k = graph.get_output_index(source)?;
            let empty = !graph.iter_connections().any(|(_, o)| o == source);
            jacks.push((graph[parts.inputs?].outputs[k].0.clone(), empty));
            if empty {
                remove_jack(graph, &index, id, Side::Input, k);
            }
        }
        AnyParameterId::Output(output) => {
            let outputs = parts.outputs?;
            // Last first, so the positions of the ones still to go hold
            let fed: Vec<usize> = (0..graph[outputs].inputs.len())
                .filter(|&k| graph.connection(graph[outputs].inputs[k].1) == Some(output))
                .collect();
            for &k in fed.iter().rev() {
                jacks.push((graph[outputs].inputs[k].0.clone(), true));
                remove_jack(graph, &index, id, Side::Output, k);
            }
            jacks.reverse();
        }
    }
    (!jacks.is_empty()).then_some((id, jacks))
}

/// A change to the list of a group's jacks on one side, made to both
/// lists that hold them, so the group's node and its Inputs or Outputs
/// agree slot for slot.
enum ListEdit {
    Rename(usize, String),
    Swap(usize, usize),
}

impl ListEdit {
    fn apply<T>(&self, list: &mut [(String, T)]) {
        match self {
            Self::Rename(k, name) => {
                if let Some(entry) = list.get_mut(*k) {
                    entry.0 = name.clone();
                }
            }
            Self::Swap(a, b) => {
                if *a < list.len() && *b < list.len() {
                    list.swap(*a, *b);
                }
            }
        }
    }
}

/// Makes the same edit to a group's jacks on one side, on its node and its
/// Inputs or Outputs.
fn edit_jacks(graph: &mut SynthGraph, index: &GroupIndex, id: GroupId, side: Side, edit: ListEdit) {
    let parts = index.parts(id);
    match side {
        Side::Input => {
            if let Some(node) = parts.node {
                edit.apply(&mut graph[node].inputs);
            }
            if let Some(inputs) = parts.inputs {
                edit.apply(&mut graph[inputs].outputs);
            }
        }
        Side::Output => {
            if let Some(node) = parts.node {
                edit.apply(&mut graph[node].outputs);
            }
            if let Some(outputs) = parts.outputs {
                edit.apply(&mut graph[outputs].inputs);
            }
        }
    }
}

/// Calls a module's pinned control `name` on the groups' faces. A blank
/// name, or the module's own short name for it, takes the face's name off.
/// Returns what the face called it and calls it now, or `None` if that's
/// the same.
pub fn name_face_control(graph: &mut SynthGraph, node_id: NodeId, param_name: &str, name: &str) -> Option<(String, String)> {
    graph.nodes.get(node_id)?;
    let own = control_label(graph, node_id, param_name);
    let data = &mut graph[node_id].user_data;
    let was = data.face_label(param_name).unwrap_or(&own).to_string();
    let name = name.trim();
    if name.is_empty() || name == own {
        data.face_labels.remove(param_name);
    } else {
        data.face_labels.insert(param_name.to_string(), name.to_string());
    }
    let now = data.face_label(param_name).unwrap_or(&own).to_string();
    (now != was).then_some((was, now))
}

/// Renames the jack a port is, on a group's node or its Inputs or Outputs.
/// A name another jack on that side has already is numbered. Returns the
/// group and the jack's old and new names, or `None` if the name is blank
/// or the same.
pub fn rename_jack(graph: &mut SynthGraph, node_id: NodeId, side: Side, port_name: &str, name: &str) -> Option<(GroupId, String, String)> {
    let index = GroupIndex::of(graph);
    let (id, group_side) = jacks_of(graph.nodes.get(node_id)?.user_data.kind, side)?;
    let (k, _) = port(graph, node_id, side, port_name)?;
    let name = name.trim();
    if name.is_empty() || name == port_name {
        return None;
    }
    let group = index.node(id)?;
    let others: Vec<String> = match group_side {
        Side::Input => graph[group].inputs.iter().map(|(n, _)| n.clone()).collect(),
        Side::Output => graph[group].outputs.iter().map(|(n, _)| n.clone()).collect(),
    };
    let others = others.into_iter().enumerate().filter(|(i, _)| *i != k).map(|(_, n)| n);
    let name = unique_names(others.chain(std::iter::once(name.to_string()))).pop()?;
    edit_jacks(graph, &index, id, group_side, ListEdit::Rename(k, name.clone()));
    Some((id, port_name.to_string(), name))
}

/// Moves the jack a port is one place up or down the group's jacks, with
/// its cables. Returns the group and the jack's name, or `None` at the end.
pub fn move_jack(graph: &mut SynthGraph, node_id: NodeId, side: Side, port_name: &str, up: bool) -> Option<(GroupId, String)> {
    let index = GroupIndex::of(graph);
    let (id, group_side) = jacks_of(graph.nodes.get(node_id)?.user_data.kind, side)?;
    let (k, _) = port(graph, node_id, side, port_name)?;
    let count = match side {
        Side::Input => graph[node_id].inputs.len(),
        Side::Output => graph[node_id].outputs.len(),
    };
    let to = if up { k.checked_sub(1)? } else { Some(k + 1).filter(|&to| to < count)? };
    edit_jacks(graph, &index, id, group_side, ListEdit::Swap(k, to));
    Some((id, port_name.to_string()))
}

/// Takes away the jack a port is, on a group's node or its Inputs or
/// Outputs, with its cables inside and out. Returns the group and the
/// jack's name.
pub fn remove_port_jack(graph: &mut SynthGraph, node_id: NodeId, side: Side, name: &str) -> Option<(GroupId, String)> {
    let index = GroupIndex::of(graph);
    let (id, group_side) = jacks_of(graph.nodes.get(node_id)?.user_data.kind, side)?;
    let (k, _) = port(graph, node_id, side, name)?;
    remove_jack(graph, &index, id, group_side, k);
    Some((id, name.to_string()))
}

/// Takes away a group's `k`th jack on one side: from its node, and the
/// matching port on its Inputs or Outputs.
fn remove_jack(graph: &mut SynthGraph, index: &GroupIndex, id: GroupId, side: Side, k: usize) {
    let parts = index.parts(id);
    let input_at = |graph: &SynthGraph, node: Option<NodeId>| node.and_then(|n| graph[n].inputs.get(k)).map(|(_, id)| *id);
    let output_at = |graph: &SynthGraph, node: Option<NodeId>| node.and_then(|n| graph[n].outputs.get(k)).map(|(_, id)| *id);
    let (input, output) = match side {
        Side::Input => (input_at(graph, parts.node), output_at(graph, parts.inputs)),
        Side::Output => (input_at(graph, parts.outputs), output_at(graph, parts.node)),
    };
    if let Some(input) = input {
        graph.remove_input_param(input);
    }
    if let Some(output) = output {
        graph.remove_output_param(output);
    }
}

/// Makes a node's ports on one side the jacks given, slot by slot: a port
/// that stays keeps its cables, renamed or retyped if need be, and ports
/// past the end go. Undo keeps cables by port position, so this puts back
/// exactly the jacks and cables a step remembers.
pub fn set_jacks(graph: &mut SynthGraph, node_id: NodeId, side: Side, jacks: &[Jack]) {
    let Some(node) = graph.nodes.get(node_id) else { return };
    match side {
        Side::Input => {
            let ports: Vec<InputId> = node.inputs.iter().map(|(_, id)| *id).collect();
            for (k, jack) in jacks.iter().enumerate() {
                match ports.get(k) {
                    Some(&input) => {
                        graph[node_id].inputs[k].0 = jack.name.clone();
                        graph.inputs[input].typ = SynthDataType::new(jack.signal);
                    }
                    None => add_jack_inputs(graph, node_id, std::slice::from_ref(jack)),
                }
            }
            for &input in ports.iter().skip(jacks.len()) {
                graph.remove_input_param(input);
            }
        }
        Side::Output => {
            let ports: Vec<OutputId> = node.outputs.iter().map(|(_, id)| *id).collect();
            for (k, jack) in jacks.iter().enumerate() {
                match ports.get(k) {
                    Some(&output) => {
                        graph[node_id].outputs[k].0 = jack.name.clone();
                        graph.outputs[output].typ = SynthDataType::new(jack.signal);
                    }
                    None => add_jack_outputs(graph, node_id, std::slice::from_ref(jack)),
                }
            }
            for &output in ports.iter().skip(jacks.len()) {
                graph.remove_output_param(output);
            }
        }
    }
}

/// Opens a group back out onto the level it sits on: its modules (and the
/// groups inside it) come out where the group's node is, and every cable
/// through its jacks becomes a cable straight from source to destination.
/// Returns the nodes that came out, or `None` if `node_id` isn't a group.
pub fn ungroup(editor: &mut SynthGraphEditorState, node_id: NodeId) -> Option<Vec<NodeId>> {
    let graph = &editor.graph;
    let node = graph.nodes.get(node_id)?;
    let id = node.user_data.kind.group()?;
    let level = node.user_data.parent;
    let index = GroupIndex::of(graph);
    let parts = index.parts(id);
    let members: Vec<NodeId> = level_nodes(graph, Some(id))
        .into_iter()
        .filter(|n| !graph[*n].user_data.kind.is_proxy())
        .collect();

    // What each jack carries, from the side it's fed
    let fed = |jack: InputId| graph.connections.get(jack).and_then(|c| c.first().copied());
    let outer_source = |k: usize| fed(graph[node_id].inputs.get(k)?.1);
    let inner_source = |k: usize| {
        let source = fed(graph[parts.outputs?].inputs.get(k)?.1)?;
        // Straight through from an input jack: whatever feeds that jack outside
        match graph.nodes.get(graph.get_output(source).node)?.user_data.kind {
            NodeKind::Inputs(_) => outer_source(graph.get_output_index(source)?),
            _ => Some(source),
        }
    };
    let mut cables = Vec::new();
    for (input, output) in graph.iter_connections() {
        let from = graph.get_output(output).node;
        let k = graph.get_output_index(output);
        let source = if Some(from) == parts.inputs && members.contains(&graph.get_input(input).node) {
            k.and_then(outer_source)
        } else if from == node_id {
            k.and_then(inner_source)
        } else {
            continue;
        };
        cables.extend(source.map(|source| (source, input)));
    }

    // The modules come out with their layout, its corner where the group was
    let corner = members.iter().filter_map(|&n| editor.node_positions.get(n).copied()).reduce(|a, b| a.min(b));
    let offset = match (corner, editor.node_positions.get(node_id)) {
        (Some(corner), Some(&at)) => at - corner,
        _ => Vec2::ZERO,
    };

    for part in [Some(node_id), parts.inputs, parts.outputs].into_iter().flatten() {
        if editor.graph.nodes.contains_key(part) {
            editor.graph.remove_node(part);
        }
        editor.node_positions.remove(part);
        editor.node_order.retain(|n| *n != part);
        editor.selected_nodes.retain(|n| *n != part);
    }
    for (output, input) in cables {
        editor.graph.add_connection(output, input, 0);
    }
    for &member in &members {
        editor.graph[member].user_data.parent = level;
        if let Some(position) = editor.node_positions.get_mut(member) {
            *position += offset;
        }
    }
    Some(members)
}

/// A miniature of what's inside a group, which its node shows on its face:
/// a box for each module in its header's colour, and the cables between
/// them. Coordinates are the editor's; the face scales them to fit.
#[derive(Clone, Debug)]
pub struct Preview {
    /// Each module's box and colour.
    pub boxes: Vec<(Rect, Color32)>,
    /// The module each box stands for, in the same order.
    pub nodes: Vec<NodeId>,
    /// Each cable's ends and colour. Cables through the group's jacks run
    /// to the edge of the miniature.
    pub wires: Vec<([Pos2; 2], Color32)>,
    /// Everything the miniature covers.
    pub bounds: Rect,
}

impl Default for Preview {
    fn default() -> Self {
        Self { boxes: Vec::new(), nodes: Vec::new(), wires: Vec::new(), bounds: Rect::NOTHING }
    }
}

/// What a group's node shows on its face, worked out once a frame.
#[derive(Clone, Debug, Default)]
pub struct Face {
    pub preview: Preview,
    /// The controls pinned to it.
    pub controls: Vec<FaceControl>,
    /// How many modules inside a learn walking it would learn (#481).
    pub learnable: usize,
}

/// A module's control pinned to a group's face.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceControl {
    /// The module the control belongs to.
    pub node: NodeId,
    pub param: String,
    /// How many groups down the module is from the face: 1 directly inside.
    pub depth: u8,
    pub kind: Control,
}

/// The kinds of control a module has, and a group's face shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// One of the knobs in its knob row.
    Knob,
    /// A dropdown or toggle, drawn among its inputs.
    Switch,
    /// A Pad's pad, played by its Press (#482).
    Pad,
    /// A Control's fader, moving with the hardware (#483).
    Fader,
}

/// The controls a module can show on a group's face, each kind in
/// parameter order: its knobs, then its dropdowns and toggles, then a Pad's
/// pad or a Control's fader.
pub fn controls(graph: &SynthGraph, node_id: NodeId) -> Vec<(&str, Control)> {
    let node = &graph[node_id];
    let knobs = node.user_data.knob_params.iter().map(|k| (k.param_name.as_str(), Control::Knob));
    let switches = node.inputs.iter().filter_map(|(name, input)| {
        let input = graph.get_input(*input);
        let switch = input.shown_inline && matches!(input.value, SynthValueType::Toggle { .. } | SynthValueType::Select { .. });
        switch.then_some((name.as_str(), Control::Switch))
    });
    let pad = (node.user_data.module_id == crate::modules::pad::ID).then_some((PAD_PRESS, Control::Pad));
    let fader = (node.user_data.module_id == crate::modules::control::ID).then_some((CONTROL_FADER, Control::Fader));
    knobs.chain(switches).chain(pad).chain(fader).collect()
}

/// The parameter a Pad's pad plays, and is pinned by.
pub const PAD_PRESS: &str = "Press";

/// The parameter a Control's fader is pinned by: its Source, which says
/// what the fader is.
pub const CONTROL_FADER: &str = "Source";

/// What a module calls one of its controls, and a group's face does too
/// until it's given a name of its own: a knob's label, or the one a dropdown
/// or toggle wears beside it ("FM", "Wave"), a pad's note, what a fader
/// hears, else the parameter's name.
pub fn control_label(graph: &SynthGraph, node_id: NodeId, param_name: &str) -> String {
    let node = &graph[node_id];
    // A pad goes by the note it hears ("C2", or "any")
    if node.user_data.module_id == crate::modules::pad::ID && param_name == PAD_PRESS {
        return super::pads_face::heard_note(super::pads_face::listen_of(graph, node_id));
    }
    // A fader by what it hears ("CC 82")
    if node.user_data.module_id == crate::modules::control::ID && param_name == CONTROL_FADER {
        return super::controls_face::listen_of(graph, node_id).label();
    }
    if let Some(knob) = node.user_data.knob_params.iter().find(|k| k.param_name == param_name) {
        if !knob.label.is_empty() {
            return knob.label.clone();
        }
    }
    match node.inputs.iter().find(|(name, _)| name == param_name).map(|(_, id)| &graph.get_input(*id).value) {
        Some(SynthValueType::Select { label, .. } | SynthValueType::Toggle { label, .. }) if !label.is_empty() => label.clone(),
        _ => param_name.to_string(),
    }
}

/// A group's name being typed, in a field on its face.
#[derive(Clone, Debug, PartialEq)]
pub struct Renaming {
    pub group: GroupId,
    pub text: String,
    /// The field hasn't been drawn yet: it takes the keys, with the old
    /// name selected so typing replaces it.
    pub fresh: bool,
}

/// The miniature of a group's insides, from where its nodes are in the
/// editor (`position`) and how big they were last drawn (`size`, if they
/// have been), at `zoom`.
pub fn preview(
    graph: &SynthGraph,
    position: impl Fn(NodeId) -> Option<Pos2>,
    size: impl Fn(NodeId) -> Option<Vec2>,
    id: GroupId,
    zoom: f32,
) -> Preview {
    let members: Vec<NodeId> = level_nodes(graph, Some(id))
        .into_iter()
        .filter(|n| !graph[*n].user_data.kind.is_proxy())
        .collect();
    let rect = |node_id: NodeId| {
        Rect::from_min_size(position(node_id).unwrap_or_default(), size(node_id).unwrap_or(NOMINAL_NODE_SIZE) * zoom)
    };
    let mut preview = Preview::default();
    let Some(bounds) = members.iter().map(|&n| rect(n)).reduce(|a, b| a.union(b)) else {
        return preview;
    };
    let pad = 24.0 * zoom;
    preview.bounds = bounds.expand2(Vec2::new(pad, 0.0));

    for &member in &members {
        let data = &graph[member].user_data;
        let color = if data.kind.group().is_some() { theme::module::GROUP } else { data.header_color() };
        preview.boxes.push((rect(member), color));
        preview.nodes.push(member);
    }
    for (input, output) in graph.iter_connections() {
        let (to, from) = (graph.get_input(input).node, graph.get_output(output).node);
        let color = graph.get_output(output).typ.signal_type().color();
        let start = if members.contains(&from) {
            rect(from).right_center()
        } else if graph[from].user_data.kind == NodeKind::Inputs(id) {
            Pos2::new(preview.bounds.left(), rect(to).center().y)
        } else {
            continue;
        };
        let end = if members.contains(&to) {
            rect(to).left_center()
        } else if graph[to].user_data.kind == NodeKind::Outputs(id) {
            Pos2::new(preview.bounds.right(), rect(from).center().y)
        } else {
            continue;
        };
        preview.wires.push(([start, end], color));
    }
    preview
}

/// The controls a group shows on its face: each one pinned to it, from
/// modules directly inside it or deeper down, nearest first, then top to
/// bottom by where the modules sit, then in parameter order.
pub fn face_controls(
    graph: &SynthGraph,
    position: impl Fn(NodeId) -> Option<Pos2>,
    index: &GroupIndex,
    id: GroupId,
) -> Vec<FaceControl> {
    let mut modules: Vec<(NodeId, u8)> = graph
        .nodes
        .iter()
        .filter(|(_, node)| node.user_data.is_module() && !node.user_data.pins.is_empty())
        .filter_map(|(node_id, node)| Some((node_id, index.depth_below(graph, id, node.user_data.parent)?)))
        .collect();
    let position = |n: NodeId| position(n).unwrap_or_default();
    modules.sort_by(|a, b| {
        (a.1, position(a.0).y, position(a.0).x).partial_cmp(&(b.1, position(b.0).y, position(b.0).x)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut shown = Vec::new();
    for (node_id, depth) in modules {
        let data = &graph[node_id].user_data;
        for (param, kind) in controls(graph, node_id) {
            if data.pin_levels(param) >= depth {
                shown.push(FaceControl { node: node_id, param: param.to_string(), depth, kind });
            }
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};
    use egui_node_graph2::NodeTemplateTrait;
    use crate::graph::{create_editor_state, SynthGraphState, SynthNodeTemplate};

    struct Rig {
        editor: SynthGraphEditorState,
        next_group: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self { editor: create_editor_state(), next_group: 1000 }
        }

        fn add(&mut self, module_id: &str, at: Pos2) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            let mut user_state = SynthGraphState::new();
            let node_id = self.editor.graph.add_node(
                template.node_graph_label(&mut user_state),
                template.user_data(&mut user_state),
                |graph, node_id| template.build_node(graph, &mut user_state, node_id),
            );
            place(&mut self.editor, node_id, at);
            node_id
        }

        fn connect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
            let graph = &mut self.editor.graph;
            let (output, input) = (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap());
            graph.add_connection(output, input, 0);
        }

        fn group(&mut self, nodes: &[NodeId], name: &str) -> NewGroup {
            let id = GroupId(self.next_group);
            self.next_group += 1;
            let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 300.0));
            group(&mut self.editor, nodes, id, name, bounds).expect("grouped")
        }

        /// Module-to-module cables, as (from node, output, to node, input) names.
        fn leaf(&self) -> Vec<(String, String, String, String)> {
            let graph = &self.editor.graph;
            let name = |n: NodeId| graph[n].label.clone();
            let mut cables: Vec<_> = leaf_cables(graph)
                .into_iter()
                .map(|(output, input)| {
                    let (from, to) = (graph.get_output(output).node, graph.get_input(input).node);
                    let out_name = graph[from].outputs.iter().find(|(_, id)| *id == output).unwrap().0.clone();
                    let in_name = graph[to].inputs.iter().find(|(_, id)| *id == input).unwrap().0.clone();
                    (name(from), out_name, name(to), in_name)
                })
                .collect();
            cables.sort();
            cables
        }
    }

    /// Note In → (Oscillator → SVF Filter → VCA ← ADSR) → Output, with the
    /// Note In's gate into the envelope and the VCA into both output sides.
    fn voice(rig: &mut Rig) -> [NodeId; 6] {
        let keys = rig.add(crate::modules::note_in::ID, pos2(0.0, 0.0));
        let osc = rig.add("osc.sine", pos2(200.0, 0.0));
        let filter = rig.add("filter.svf", pos2(400.0, 0.0));
        let env = rig.add("mod.adsr", pos2(400.0, 200.0));
        let vca = rig.add("util.vca", pos2(600.0, 0.0));
        let out = rig.add("output.audio", pos2(800.0, 0.0));
        rig.connect(keys, "Pitch", osc, "V/Oct");
        rig.connect(keys, "Gate", env, "Gate");
        rig.connect(osc, "Out", filter, "In");
        rig.connect(filter, "LowPass", vca, "In");
        rig.connect(env, "Out", vca, "CV");
        rig.connect(vca, "Out", out, "Left");
        rig.connect(vca, "Out", out, "Right");
        [keys, osc, filter, env, vca, out]
    }

    #[test]
    fn grouping_makes_jacks_from_crossing_cables_and_keeps_the_sound() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let before = rig.leaf();
        let new = rig.group(&[osc, filter, env, vca], "Voice");

        let graph = &rig.editor.graph;
        let names = |jacks: Vec<Jack>| jacks.into_iter().map(|j| j.name).collect::<Vec<_>>();
        // The oscillator sits above the envelope, so its jack comes first
        assert_eq!(names(input_jacks(graph, new.node)), ["V/Oct", "Gate"]);
        // Two cables from the VCA's one output make one jack
        assert_eq!(names(output_jacks(graph, new.node)), ["Out"]);
        assert_eq!(input_jacks(graph, new.node)[1].signal, SignalType::Gate);
        assert_eq!(graph[new.inputs].outputs.len(), 2);
        assert_eq!(graph[new.outputs].inputs.len(), 1);
        assert_eq!(graph[osc].user_data.parent, Some(GroupId(1000)));
        assert_eq!(graph[new.node].user_data.parent, None);

        // The engine hears the same patch
        assert_eq!(rig.leaf(), before);
    }

    #[test]
    fn ungrouping_restores_the_cables_and_the_layout() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let before = rig.leaf();
        let positions: Vec<Pos2> = [osc, filter, env, vca].iter().map(|&n| rig.editor.node_positions[n]).collect();
        let cables_before = rig.editor.graph.iter_connections().count();

        let new = rig.group(&[osc, filter, env, vca], "Voice");
        let members = ungroup(&mut rig.editor, new.node).unwrap();
        assert_eq!(members.len(), 4);
        assert_eq!(rig.leaf(), before);
        // Every cable is a plain one again, and nothing of the group is left
        assert_eq!(rig.editor.graph.iter_connections().count(), cables_before);
        assert_eq!(rig.editor.graph.nodes.len(), 6);
        assert_eq!(rig.editor.node_order.len(), 6);
        assert!(rig.editor.graph.nodes.values().all(|n| n.user_data.parent.is_none()));
        let after: Vec<Pos2> = [osc, filter, env, vca].iter().map(|&n| rig.editor.node_positions[n]).collect();
        assert_eq!(after, positions);
    }

    #[test]
    fn groups_nest_and_cables_follow_every_hop() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let before = rig.leaf();
        let inner = rig.group(&[osc, filter], "Tone");
        let outer = rig.group(&[inner.node, env, vca], "Voice");
        assert_eq!(rig.leaf(), before);

        let graph = &rig.editor.graph;
        let index = GroupIndex::of(graph);
        let (tone, voice) = (GroupId(1000), GroupId(1001));
        assert_eq!(index.path(graph, Some(tone)), Some(vec![voice, tone]));
        assert_eq!(index.depth_below(graph, voice, graph[osc].user_data.parent), Some(2));
        assert_eq!(index.depth_below(graph, tone, graph[env].user_data.parent), None);
        // The inner group's three nodes and its two modules, and the outer
        // group's Inputs and Outputs, the envelope and the VCA
        assert_eq!(descendants(graph, &index, voice).len(), 3 + 2 + 2 + 2);

        // Taking the outer group apart leaves the inner one whole
        ungroup(&mut rig.editor, outer.node).unwrap();
        assert_eq!(rig.leaf(), before);
        assert_eq!(rig.editor.graph[inner.node].user_data.parent, None);
        ungroup(&mut rig.editor, inner.node).unwrap();
        assert_eq!(rig.leaf(), before);
        assert_eq!(rig.editor.graph.nodes.len(), 6);
    }

    #[test]
    fn an_unplugged_jack_carries_nothing_and_a_new_cable_reaches_inside() {
        let mut rig = Rig::new();
        let [keys, osc, filter, _, _, _] = voice(&mut rig);
        let new = rig.group(&[osc, filter], "Tone");
        let graph = &mut rig.editor.graph;
        let pitch_jack = graph[new.node].get_input("V/Oct").unwrap();
        let keys_pitch = graph[keys].get_output("Pitch").unwrap();
        assert!(graph.remove_connection(pitch_jack, keys_pitch));
        assert!(!rig.leaf().iter().any(|c| c.3 == "V/Oct"));

        // Plugging the Note In's gate into the jack instead reaches the oscillator
        let graph = &mut rig.editor.graph;
        let keys_gate = graph[keys].get_output("Gate").unwrap();
        graph.add_connection(keys_gate, pitch_jack, 0);
        assert!(rig.leaf().contains(&("Note In".into(), "Gate".into(), "Oscillator".into(), "V/Oct".into())));
    }

    #[test]
    fn a_face_control_takes_a_name_and_a_blank_one_gives_it_back() {
        let mut rig = Rig::new();
        let [_, osc, filter, ..] = voice(&mut rig);
        rig.editor.graph[filter].user_data.pins.insert("Cutoff".into(), 1);
        rig.editor.graph[osc].user_data.pins.insert("Waveform".into(), 1);
        rig.group(&[osc, filter], "Tone");
        let graph = &mut rig.editor.graph;
        let own = control_label(graph, filter, "Cutoff");

        let named = name_face_control(graph, filter, "Cutoff", "  Bright ");
        assert_eq!(named, Some((own.clone(), "Bright".to_string())));
        assert_eq!(graph[filter].user_data.face_label("Cutoff"), Some("Bright"));
        // The same name again changes nothing
        assert_eq!(name_face_control(graph, filter, "Cutoff", "Bright"), None);
        // A blank name, or its own, goes back to the module's
        assert_eq!(name_face_control(graph, filter, "Cutoff", " "), Some(("Bright".to_string(), own.clone())));
        assert!(graph[filter].user_data.face_labels.is_empty());
        name_face_control(graph, filter, "Cutoff", "Bright");
        name_face_control(graph, filter, "Cutoff", &own);
        assert!(graph[filter].user_data.face_labels.is_empty());

        // A dropdown is named the same way
        name_face_control(graph, osc, "Waveform", "Shape");
        assert_eq!(graph[osc].user_data.face_label("Waveform"), Some("Shape"));
    }

    #[test]
    fn jack_names_repeat_with_numbers() {
        assert_eq!(
            unique_names(["In", "In", "Cutoff", "In"].map(String::from)),
            ["In", "In 2", "Cutoff", "In 3"]
        );
    }

    #[test]
    fn pinned_controls_show_on_the_groups_they_reach() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        rig.editor.graph[filter].user_data.pins.insert("Cutoff".into(), 1);
        rig.editor.graph[osc].user_data.pins.insert("Octave".into(), 2);
        // A dropdown shows as well as a knob; a name that's neither doesn't
        rig.editor.graph[osc].user_data.pins.insert("Waveform".into(), 1);
        rig.editor.graph[osc].user_data.pins.insert("Wobble".into(), 1);
        rig.group(&[osc, filter], "Tone");
        let outer = rig.group(&[env, vca], "Amp");
        let graph = &rig.editor.graph;
        let index = GroupIndex::of(graph);
        let positions = &rig.editor.node_positions;
        let knobs = |id| face_controls(graph, |n| positions.get(n).copied(), &index, id);
        // Oscillator (left) before the filter, both directly inside Tone
        let knob = |node, param: &str| FaceControl { node, param: param.to_string(), depth: 1, kind: Control::Knob };
        let switch = |node, param: &str| FaceControl { kind: Control::Switch, ..knob(node, param) };
        assert_eq!(knobs(GroupId(1000)), [knob(osc, "Octave"), switch(osc, "Waveform"), knob(filter, "Cutoff")]);
        assert!(knobs(GroupId(1001)).is_empty());
        let _ = outer;
    }

    #[test]
    fn a_pinned_pad_shows_on_its_group_by_its_note() {
        let mut rig = Rig::new();
        let kick = rig.add(crate::modules::pad::ID, pos2(0.0, 0.0));
        let snare = rig.add(crate::modules::pad::ID, pos2(250.0, 0.0));
        let osc = rig.add("osc.sine", pos2(0.0, 300.0));
        // A Pad offers its pad, and nothing else does
        assert_eq!(controls(&rig.editor.graph, kick).last(), Some(&(PAD_PRESS, Control::Pad)));
        assert!(!controls(&rig.editor.graph, osc).iter().any(|(_, kind)| *kind == Control::Pad));
        rig.editor.graph[kick].user_data.pins.insert(PAD_PRESS.into(), 1);
        rig.group(&[kick, snare, osc], "Kit");
        let graph = &rig.editor.graph;
        let positions = &rig.editor.node_positions;
        let shown = face_controls(graph, |n| positions.get(n).copied(), &GroupIndex::of(graph), GroupId(1000));
        assert_eq!(shown, [FaceControl { node: kick, param: PAD_PRESS.into(), depth: 1, kind: Control::Pad }]);
        // Named for its note until it's given a name of its own
        assert_eq!(control_label(graph, kick, PAD_PRESS), "C2");
        assert_eq!(name_face_control(&mut rig.editor.graph, kick, PAD_PRESS, "Kick"), Some(("C2".into(), "Kick".into())));
    }

    #[test]
    fn a_pinned_fader_shows_on_its_group_by_what_it_hears() {
        let mut rig = Rig::new();
        let fader = rig.add(crate::modules::control::ID, pos2(0.0, 0.0));
        let other = rig.add(crate::modules::control::ID, pos2(210.0, 0.0));
        // A Control offers its fader beside its Slew knob, and nothing else
        let offered = controls(&rig.editor.graph, fader);
        assert_eq!(offered.last(), Some(&(CONTROL_FADER, Control::Fader)));
        assert!(offered.iter().all(|(name, kind)| *kind != Control::Switch && (*kind == Control::Fader || *name == "Slew")));
        rig.editor.graph[fader].user_data.pins.insert(CONTROL_FADER.into(), 1);
        rig.group(&[fader, other], "Faders");
        let graph = &rig.editor.graph;
        let positions = &rig.editor.node_positions;
        let shown = face_controls(graph, |n| positions.get(n).copied(), &GroupIndex::of(graph), GroupId(1000));
        assert_eq!(shown, [FaceControl { node: fader, param: CONTROL_FADER.into(), depth: 1, kind: Control::Fader }]);
        assert_eq!(control_label(graph, fader, CONTROL_FADER), "Mod", "a new Control hears the mod wheel");
    }

    #[test]
    fn a_port_shown_on_its_group_gets_a_jack_that_carries_both_ways() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let lfo = rig.add("mod.lfo", pos2(0.0, 400.0));
        let scope = rig.add("util.oscilloscope", pos2(900.0, 400.0));
        let new = rig.group(&[osc, filter, env, vca], "Voice");
        let index = GroupIndex::of(&rig.editor.graph);
        let actions = |rig: &Rig, node, side, name| jack_actions(&rig.editor.graph, &index, node, side, name);
        assert_eq!(actions(&rig, filter, Side::Input, "Cutoff"), [JackAction::Show]);
        // Fed by the oscillator inside, so it can't take a jack too
        assert_eq!(actions(&rig, filter, Side::Input, "In"), [JackAction::Taken]);
        // A knob with no jack has no port to show
        assert!(actions(&rig, filter, Side::Input, "Drive").is_empty());
        assert_eq!(actions(&rig, new.inputs, Side::Output, "Gate"), [JackAction::Jack { slot: 1, count: 2 }]);

        let graph = &mut rig.editor.graph;
        assert_eq!(show_port(graph, filter, Side::Input, "Cutoff"), Some((GroupId(1000), "Cutoff".to_string())));
        assert_eq!(show_port(graph, env, Side::Output, "Out"), Some((GroupId(1000), "Out 2".to_string())));
        let names = |jacks: Vec<Jack>| jacks.into_iter().map(|j| j.name).collect::<Vec<_>>();
        assert_eq!(names(input_jacks(graph, new.node)), ["V/Oct", "Gate", "Cutoff"]);
        assert_eq!(names(output_jacks(graph, new.node)), ["Out", "Out 2"]);
        assert_eq!(names(output_jacks(graph, new.inputs)), ["V/Oct", "Gate", "Cutoff"]);
        assert_eq!(names(input_jacks(graph, new.outputs)), ["Out", "Out 2"]);

        // Plugged in outside, they reach the modules inside
        rig.connect(lfo, "Out", new.node, "Cutoff");
        rig.connect(new.node, "Out 2", scope, "In 1");
        let leaf = rig.leaf();
        assert!(leaf.contains(&("LFO".into(), "Out".into(), "SVF Filter".into(), "Cutoff".into())), "{leaf:?}");
        assert!(leaf.contains(&("ADSR Envelope".into(), "Out".into(), "Oscilloscope".into(), "In 1".into())), "{leaf:?}");
        let index = GroupIndex::of(&rig.editor.graph);
        assert_eq!(jack_actions(&rig.editor.graph, &index, filter, Side::Input, "Cutoff"), [JackAction::Hide]);
        assert_eq!(jack_actions(&rig.editor.graph, &index, env, Side::Output, "Out"), [JackAction::Hide]);

        // Hidden again, the jacks go with their cables outside
        let graph = &mut rig.editor.graph;
        assert_eq!(hide_port(graph, filter, Side::Input, "Cutoff"), Some((GroupId(1000), vec![("Cutoff".to_string(), true)])));
        assert_eq!(remove_port_jack(graph, new.outputs, Side::Input, "Out 2"), Some((GroupId(1000), "Out 2".to_string())));
        assert_eq!(names(input_jacks(graph, new.node)), ["V/Oct", "Gate"]);
        assert_eq!(names(output_jacks(graph, new.node)), ["Out"]);
        assert_eq!(graph[new.inputs].outputs.len(), 2);
        assert_eq!(graph[new.outputs].inputs.len(), 1);
        assert!(!rig.leaf().iter().any(|c| c.0 == "LFO" || c.2 == "Oscilloscope"));
    }

    #[test]
    fn hiding_one_port_of_a_shared_jack_keeps_the_jack() {
        let mut rig = Rig::new();
        let [keys, osc, _, env, vca, _] = voice(&mut rig);
        let env2 = rig.add("mod.adsr", pos2(400.0, 400.0));
        rig.connect(keys, "Gate", env2, "Gate");
        rig.connect(env2, "Out", vca, "CV");
        let new = rig.group(&[osc, env, env2, vca], "Voice");
        // The Note In's Gate feeds both envelopes through one jack
        let graph = &mut rig.editor.graph;
        assert_eq!(hide_port(graph, env2, Side::Input, "Gate"), Some((GroupId(1000), vec![("Gate".to_string(), false)])));
        assert!(graph[new.node].get_input("Gate").is_ok());
        assert!(rig.leaf().contains(&("Note In".into(), "Gate".into(), "ADSR Envelope".into(), "Gate".into())));
    }

    #[test]
    fn a_group_inside_a_group_can_show_its_jacks_on_the_outer_one() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let inner = rig.group(&[osc, filter], "Tone");
        let outer = rig.group(&[inner.node, env, vca], "Voice");
        // Tone's own jack can go, or be shown on Voice
        let index = GroupIndex::of(&rig.editor.graph);
        let graph = &mut rig.editor.graph;
        let _ = show_port(graph, filter, Side::Input, "Cutoff").unwrap();
        assert_eq!(jack_actions(graph, &index, inner.node, Side::Input, "Cutoff"), [JackAction::Jack { slot: 1, count: 2 }, JackAction::Show]);
        assert_eq!(show_port(graph, inner.node, Side::Input, "Cutoff"), Some((GroupId(1001), "Cutoff".to_string())));
        assert!(graph[outer.node].get_input("Cutoff").is_ok());
    }

    #[test]
    fn a_group_is_found_by_its_name_or_its_path() {
        let mut rig = Rig::new();
        let [keys, osc, filter, env, vca, out] = voice(&mut rig);
        let inner = rig.group(&[osc, filter], "Tone");
        rig.group(&[inner.node, env, vca], "Voice");
        let (tone, voice) = (GroupId(1000), GroupId(1001));
        let find = |rig: &Rig, level, wanted| {
            find_group(&rig.editor.graph, |n| rig.editor.node_positions.get(n).copied(), level, wanted)
        };
        assert_eq!(find(&rig, None, "Voice"), Ok(voice));
        // Not on the level on show, but the only one so called
        assert_eq!(find(&rig, None, "Tone"), Ok(tone));
        assert_eq!(find(&rig, None, "Voice/Tone"), Ok(tone));
        assert_eq!(find(&rig, None, " Voice / Tone "), Ok(tone));
        assert!(find(&rig, None, "Tone/Voice").is_err());
        assert!(find(&rig, None, "Drums").is_err());

        // Another called Tone: the one on show, or a path, picks
        let left = rig.group(&[keys], "Tone");
        let top_tone = GroupId(1002);
        assert_eq!(find(&rig, None, "Tone"), Ok(top_tone));
        assert_eq!(find(&rig, Some(voice), "Tone"), Ok(tone));
        assert_eq!(find(&rig, None, "Voice/Tone"), Ok(tone));
        let err = find(&rig, Some(tone), "Tone").unwrap_err();
        assert!(err.contains("2 groups called Tone"), "{err}");

        // Two side by side: counted down the page, then across
        let right = rig.group(&[out], "Tone");
        rig.editor.node_positions.insert(left.node, pos2(0.0, 300.0));
        rig.editor.node_positions.insert(right.node, pos2(500.0, 0.0));
        let top_right = GroupId(1003);
        assert!(find(&rig, None, "Tone").unwrap_err().contains("Tone#2"));
        assert_eq!(find(&rig, None, "Tone#1"), Ok(top_right));
        assert_eq!(find(&rig, None, "Tone#2"), Ok(top_tone));
        assert_eq!(find(&rig, None, "Voice/Tone#1"), Ok(tone));
        assert!(find(&rig, None, "Tone#3").is_err());
    }

    #[test]
    fn set_jacks_keeps_the_ports_that_stay() {
        let mut rig = Rig::new();
        let [_, osc, filter, env, vca, _] = voice(&mut rig);
        let new = rig.group(&[osc, filter, env, vca], "Voice");
        let graph = &mut rig.editor.graph;
        let gate = graph[new.node].get_input("Gate").unwrap();
        let jack = |name: &str, signal| Jack { name: name.into(), signal };
        let jacks = [jack("V/Oct", SignalType::Control), jack("Gate", SignalType::Gate), jack("Cutoff", SignalType::Control)];
        set_jacks(graph, new.node, Side::Input, &jacks);
        assert_eq!(input_jacks(graph, new.node), jacks);
        assert_eq!(graph[new.node].get_input("Gate").unwrap(), gate);
        set_jacks(graph, new.node, Side::Input, &jacks[..1]);
        assert_eq!(input_jacks(graph, new.node), jacks[..1]);
    }

    #[test]
    fn jacks_rename_and_move_with_their_cables() {
        let mut rig = Rig::new();
        let [keys, osc, filter, env, vca, _] = voice(&mut rig);
        let new = rig.group(&[osc, filter, env, vca], "Voice");
        let before = rig.leaf();
        let names = |rig: &Rig| input_jacks(&rig.editor.graph, new.node).into_iter().map(|j| j.name).collect::<Vec<_>>();
        let inside = |rig: &Rig| output_jacks(&rig.editor.graph, new.inputs).into_iter().map(|j| j.name).collect::<Vec<_>>();

        // Renamed from outside, Inputs inside follows
        let graph = &mut rig.editor.graph;
        assert_eq!(rename_jack(graph, new.node, Side::Input, "V/Oct", " Pitch "), Some((GroupId(1000), "V/Oct".into(), "Pitch".into())));
        assert_eq!(names(&rig), ["Pitch", "Gate"]);
        assert_eq!(inside(&rig), ["Pitch", "Gate"]);
        // A name that's taken is numbered; a blank one changes nothing
        let graph = &mut rig.editor.graph;
        assert_eq!(rename_jack(graph, new.inputs, Side::Output, "Gate", "Pitch").map(|r| r.2), Some("Pitch 2".into()));
        assert_eq!(rename_jack(graph, new.node, Side::Input, "Pitch 2", "  "), None);
        assert_eq!(rename_jack(graph, new.node, Side::Input, "Pitch 2", "Gate").map(|r| r.2), Some("Gate".into()));

        // Moved, both lists swap, and every module hears what it did
        let graph = &mut rig.editor.graph;
        assert_eq!(move_jack(graph, new.node, Side::Input, "Gate", true), Some((GroupId(1000), "Gate".into())));
        assert_eq!(names(&rig), ["Gate", "Pitch"]);
        assert_eq!(inside(&rig), ["Gate", "Pitch"]);
        let graph = &mut rig.editor.graph;
        assert_eq!(move_jack(graph, new.node, Side::Input, "Gate", true), None);
        assert_eq!(move_jack(graph, new.inputs, Side::Output, "Pitch", false), None);
        assert_eq!(rig.leaf(), before);
        let graph = &rig.editor.graph;
        assert_eq!(graph.connection(graph[new.node].get_input("Pitch").unwrap()), graph[keys].get_output("Pitch").ok());
    }
}
