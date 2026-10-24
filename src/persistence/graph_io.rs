//! Moving patches in and out of the editor graph.
//!
//! [`stage_patch`] builds a whole patch into a fresh graph without touching
//! anything live. Callers swap the result in only once it has been built, so
//! a bad patch can never leave a half-loaded graph behind. [`capture_patch`]
//! goes the other way, for saving.
//!
//! Both the editor and headless compilation load through [`stage_patch`], so
//! they agree on how names, IDs and problems are resolved.

use std::collections::{HashMap, HashSet};

use egui_node_graph2::{InputId, NodeId, NodeTemplateTrait, OutputId};

use crate::dsp::SignalType;
use crate::engine::NodeId as EngineNodeId;
use crate::graph::groups::{self, GroupIndex, Jack};
use crate::graph::loops;
use crate::graph::{
    port_mapping, validate_connection, GroupId, NodeKind, SynthGraph, SynthGraphState, SynthNodeTemplate,
    SynthValueType,
};

use super::{
    ConnectionData, GroupData, JackData, MidiMapping, NamedParameter, NodeData, ParameterValue, Patch, PatchError,
    PATCH_VERSION,
};

/// A module of a staged patch.
#[derive(Debug, Clone, Copy)]
pub struct StagedNode {
    /// The node's ID in the patch file.
    pub patch_id: u64,
    /// The node's ID in [`StagedPatch::graph`].
    pub graph_id: NodeId,
    /// The template the node was built from.
    pub template: SynthNodeTemplate,
    /// Saved editor position.
    pub position: (f32, f32),
    /// Inside one of the patch's groups, rather than on its top level.
    pub nested: bool,
}

/// One of the nodes a staged group is made of: the group's own node, or
/// its Inputs or Outputs.
#[derive(Debug, Clone, Copy)]
pub struct StagedPart {
    pub graph_id: NodeId,
    /// Saved editor position.
    pub position: (f32, f32),
    /// Inside one of the patch's groups, rather than on its top level.
    pub nested: bool,
}

/// A patch built into its own graph, ready to be swapped in.
pub struct StagedPatch {
    /// The patch's nodes, parameter values and connections.
    pub graph: SynthGraph,
    /// Every module that was built, in patch order, a group's modules after
    /// the ones beside the group. Skipped modules are missing.
    pub nodes: Vec<StagedNode>,
    /// The nodes of every group that was built. Their group IDs are only
    /// the staging's own; see [`renumber_groups`].
    pub parts: Vec<StagedPart>,
    /// MIDI mappings whose `param_index` has been resolved by name, but whose
    /// `node_id` is still the patch node ID. See [`StagedPatch::remap_midi_mappings`].
    midi_mappings: Vec<MidiMapping>,
    /// Things that were skipped rather than failing the load.
    pub warnings: Vec<String>,
}

impl StagedPatch {
    /// The staged graph node for a patch node ID, or `None` if it was skipped.
    pub fn graph_id(&self, patch_id: u64) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.patch_id == patch_id).map(|n| n.graph_id)
    }

    /// The patch's MIDI mappings, retargeted at the engine IDs the caller
    /// assigned to the staged nodes.
    ///
    /// Patch node IDs are whatever engine IDs the nodes had when the patch was
    /// saved, so they can't be used as-is: after a load they'd point at the
    /// wrong nodes, or none.
    pub fn remap_midi_mappings(
        &self,
        engine_id: impl Fn(NodeId) -> Option<EngineNodeId>,
    ) -> Vec<MidiMapping> {
        self.midi_mappings
            .iter()
            .filter_map(|mapping| {
                let node_id = engine_id(self.graph_id(mapping.node_id)?)?;
                Some(MidiMapping { node_id, ..mapping.clone() })
            })
            .collect()
    }
}

/// Points the scenes of a patch's Scenes modules, which name modules by
/// their IDs in the patch, at the engine IDs `engine_id` gave them. Call it
/// once a staged or merged patch's modules have theirs.
pub fn remap_scenes(graph: &mut SynthGraph, nodes: &[StagedNode], engine_id: impl Fn(NodeId) -> Option<EngineNodeId>) {
    let ids: HashMap<u64, EngineNodeId> =
        nodes.iter().filter_map(|n| Some((n.patch_id, engine_id(n.graph_id)?))).collect();
    for node in nodes {
        if let Some(data) = graph.nodes.get_mut(node.graph_id).map(|n| &mut n.user_data) {
            if !data.scenes.is_empty() {
                crate::graph::scenes::remap(&mut data.scenes, |id| ids.get(&id).copied());
            }
        }
    }
}

/// Gives every group in `graph` a new ID from `allocate`: the ones a
/// staging made up are only good within the staged graph.
pub fn renumber_groups(graph: &mut SynthGraph, mut allocate: impl FnMut() -> GroupId) {
    let mut ids: Vec<GroupId> = GroupIndex::of(graph).ids().collect();
    ids.sort();
    let new: HashMap<GroupId, GroupId> = ids.into_iter().map(|id| (id, allocate())).collect();
    groups::renumber(graph, &new);
}

/// Builds a patch into a fresh graph.
///
/// Only an incompatible version fails the load. Everything else that can't be
/// restored is skipped and reported in [`StagedPatch::warnings`]:
///
/// - nodes with an unknown module ID
/// - parameters with an unknown name (parameters missing from the patch keep
///   their defaults)
/// - connections to skipped nodes, missing ports, or of incompatible types
/// - connections that would close a loop, which the engine would turn away
/// - MIDI mappings to skipped nodes or unknown parameters
pub fn stage_patch(patch: &Patch) -> Result<StagedPatch, PatchError> {
    check_version(patch)?;
    let mut graph = SynthGraph::default();
    let mut next = 0;
    let built = build_into(&mut graph, patch, None, &mut || {
        next += 1;
        GroupId(next)
    });
    Ok(StagedPatch {
        graph,
        nodes: built.nodes,
        parts: built.parts,
        midi_mappings: built.midi_mappings,
        warnings: built.warnings,
    })
}

/// What [`merge_patch`] added.
pub struct Merged {
    /// The modules, in patch order.
    pub nodes: Vec<StagedNode>,
    /// The groups' nodes.
    pub parts: Vec<StagedPart>,
    /// Anything that was skipped.
    pub warnings: Vec<String>,
}

/// Adds a patch's nodes and the connections between them to a graph that
/// already has nodes, e.g. to paste copied modules, on the level `parent`.
/// New groups take their IDs from `allocate`. Returns what was added and
/// anything that was skipped, as [`stage_patch`] does.
///
/// MIDI mappings are left behind: one controller turning both the original
/// and the copy is rarely what anyone wants.
pub fn merge_patch(
    graph: &mut SynthGraph,
    patch: &Patch,
    parent: Option<GroupId>,
    allocate: &mut dyn FnMut() -> GroupId,
) -> Result<Merged, PatchError> {
    check_version(patch)?;
    let built = build_into(graph, patch, parent, allocate);
    Ok(Merged { nodes: built.nodes, parts: built.parts, warnings: built.warnings })
}

fn check_version(patch: &Patch) -> Result<(), PatchError> {
    if patch.is_compatible() {
        Ok(())
    } else {
        Err(PatchError::IncompatibleVersion { found: patch.version, expected: PATCH_VERSION })
    }
}

/// What [`build_into`] built.
struct Built {
    nodes: Vec<StagedNode>,
    parts: Vec<StagedPart>,
    midi_mappings: Vec<MidiMapping>,
    warnings: Vec<String>,
}

/// Builds a patch's nodes, groups, connections and MIDI mappings into
/// `graph`, its top level in `parent`. Connections only join nodes built
/// here, never ones already in the graph.
fn build_into(
    graph: &mut SynthGraph,
    patch: &Patch,
    parent: Option<GroupId>,
    allocate: &mut dyn FnMut() -> GroupId,
) -> Built {
    let mut builder = Builder {
        graph,
        allocate,
        // Templates don't keep anything in the user state while building
        user_state: SynthGraphState::new(),
        nodes: Vec::with_capacity(patch.nodes.len()),
        parts: Vec::new(),
        modules: HashMap::new(),
        warnings: Vec::new(),
    };
    builder.level(&patch.nodes, &patch.groups, &patch.connections, parent, None);
    builder.skip_loops();

    let mut midi_mappings = Vec::with_capacity(patch.midi_mappings.len());
    for mapping in &patch.midi_mappings {
        let Some(&graph_id) = builder.modules.get(&mapping.node_id) else {
            builder.warnings.push(format!(
                "Dropped MIDI CC {} mapping: node {} wasn't loaded",
                mapping.cc_number, mapping.node_id
            ));
            continue;
        };
        // A Scenes pad's mapping names its pad, not a parameter (#423)
        if mapping.scene.is_some() {
            midi_mappings.push(mapping.clone());
            continue;
        }
        let Some(param_index) = parameter_index(builder.graph, graph_id, &mapping.param_name) else {
            builder.warnings.push(format!(
                "Dropped MIDI CC {} mapping: no parameter '{}'",
                mapping.cc_number, mapping.param_name
            ));
            continue;
        };
        midi_mappings.push(MidiMapping { param_index, ..mapping.clone() });
    }

    Built { nodes: builder.nodes, parts: builder.parts, midi_mappings, warnings: builder.warnings }
}

/// A group's inside being built: its ID in the patch, and its Inputs and
/// Outputs, which cables to and from that ID mean.
#[derive(Clone, Copy)]
struct Inside {
    patch_id: u64,
    inputs: NodeId,
    outputs: NodeId,
}

struct Builder<'a> {
    graph: &'a mut SynthGraph,
    allocate: &'a mut dyn FnMut() -> GroupId,
    user_state: SynthGraphState,
    nodes: Vec<StagedNode>,
    parts: Vec<StagedPart>,
    /// Every module built, by patch ID, for MIDI mappings.
    modules: HashMap<u64, NodeId>,
    warnings: Vec<String>,
}

impl Builder<'_> {
    /// Builds one level of the patch: its modules, its groups (and their
    /// insides), and the cables between them.
    fn level(
        &mut self,
        nodes: &[NodeData],
        groups: &[GroupData],
        connections: &[ConnectionData],
        parent: Option<GroupId>,
        inside: Option<Inside>,
    ) {
        let nested = inside.is_some();
        // The node each patch ID stands for on this level
        let mut here: HashMap<u64, NodeId> = HashMap::new();

        for node_data in nodes {
            let Some(template) = SynthNodeTemplate::from_module_id(&node_data.module_id) else {
                self.warnings.push(format!(
                    "Skipped node {}: unknown module '{}'",
                    node_data.id, node_data.module_id
                ));
                continue;
            };

            let user_state = &mut self.user_state;
            let graph_id = self.graph.add_node(
                template.node_graph_label(user_state),
                template.user_data(user_state),
                |graph, node_id| template.build_node(graph, user_state, node_id),
            );
            restore_parameters(self.graph, graph_id, node_data, &mut self.warnings);
            let user_data = &mut self.graph[graph_id].user_data;
            user_data.bypassed = node_data.bypassed && user_data.bypassable;
            user_data.parent = parent;
            user_data.pins = node_data.pinned.clone();
            user_data.face_labels = node_data.face_labels.clone();
            user_data.file = node_data.file.clone();
            user_data.labels = node_data.labels.clone();
            user_data.slice_markers = node_data.slice_markers.clone();
            // Still under the patch's module IDs, until they get engine IDs
            user_data.scenes = node_data.scenes.clone();

            here.insert(node_data.id, graph_id);
            self.modules.insert(node_data.id, graph_id);
            self.nodes.push(StagedNode {
                patch_id: node_data.id,
                graph_id,
                template,
                position: node_data.position,
                nested,
            });
        }

        for group in groups {
            let id = (self.allocate)();
            let inputs = self.jacks(&group.name, &group.inputs);
            let outputs = self.jacks(&group.name, &group.outputs);
            let node = groups::add_group_node(self.graph, id, &group.name, parent, &inputs, &outputs);
            let inputs_node = groups::add_inputs_node(self.graph, id, &inputs);
            let outputs_node = groups::add_outputs_node(self.graph, id, &outputs);
            self.parts.extend([
                StagedPart { graph_id: node, position: group.position, nested },
                StagedPart { graph_id: inputs_node, position: group.inputs_position, nested: true },
                StagedPart { graph_id: outputs_node, position: group.outputs_position, nested: true },
            ]);
            here.insert(group.id, node);

            let inside = Inside { patch_id: group.id, inputs: inputs_node, outputs: outputs_node };
            self.level(&group.nodes, &group.groups, &group.connections, Some(id), Some(inside));
        }

        for conn in connections {
            // Inside a group, its own ID is its jacks: in from Inputs, out to Outputs
            let from = match inside {
                Some(inside) if conn.from_node == inside.patch_id => Some(inside.inputs),
                _ => here.get(&conn.from_node).copied(),
            };
            let to = match inside {
                Some(inside) if conn.to_node == inside.patch_id => Some(inside.outputs),
                _ => here.get(&conn.to_node).copied(),
            };
            let (Some(from), Some(to)) = (from, to) else {
                self.warnings.push(format!(
                    "Skipped connection {} -> {}: a node wasn't loaded",
                    conn.from_node, conn.to_node
                ));
                continue;
            };

            let graph = &mut *self.graph;
            let output = graph.nodes[from]
                .outputs
                .iter()
                .find(|(name, _)| *name == conn.from_port)
                .map(|(_, id)| *id);
            let input = graph.nodes[to]
                .inputs
                .iter()
                .find(|(name, id)| *name == conn.to_port && port_mapping::is_connectable(graph.get_input(*id).kind))
                .map(|(_, id)| *id);
            // As the loop warning names a cable: each end's module, patch ID and port
            let cable = |graph: &SynthGraph| {
                format!(
                    "{} (node {}) '{}' -> {} (node {}) '{}'",
                    graph[from].label, conn.from_node, conn.from_port, graph[to].label, conn.to_node, conn.to_port
                )
            };
            let (Some(output), Some(input)) = (output, input) else {
                let missing = match output {
                    None => format!("{} has no output '{}'", graph[from].label, conn.from_port),
                    Some(_) => format!("{} has no input '{}'", graph[to].label, conn.to_port),
                };
                self.warnings.push(format!("Skipped connection {}: {missing}", cable(graph)));
                continue;
            };

            let result = validate_connection(graph.get_output(output).typ.0, graph.get_input(input).typ.0);
            if let Some(error) = result.error_message() {
                self.warnings.push(format!("Skipped connection {}: {error}", cable(graph)));
                continue;
            }

            graph.add_connection(output, input, 0);
        }
    }

    /// Takes out each cable that would close a loop, which the engine would
    /// turn away, so the editor never shows a cable that isn't heard. Which
    /// of a loop's cables goes is down to their order: the graph's own
    /// first, then the patch's in [`patch_order`].
    fn skip_loops(&mut self) {
        let graph = &*self.graph;
        let patch_ids: HashMap<NodeId, u64> = self.modules.iter().map(|(&patch_id, &node)| (node, patch_id)).collect();
        let mut cables = groups::leaf_cables(graph);
        patch_order(graph, &mut cables, &patch_ids);
        let mut skipped = Vec::new();
        for found in loops::closing(graph, &cables) {
            let (output, input) = cables[found.index];
            let (from, to) = (graph.get_output(output).node, graph.get_input(input).node);
            let end = |node: NodeId, port: Option<String>| {
                let id = patch_ids.get(&node).map_or("?".to_string(), u64::to_string);
                format!("{} (node {}) '{}'", graph[node].label, id, port.unwrap_or_default())
            };
            self.warnings.push(format!(
                "Skipped connection {} -> {}: it would close the loop {}",
                end(from, port_name(&graph[from].outputs, output)),
                end(to, port_name(&graph[to].inputs, input)),
                loops::describe(graph, &found.route)
            ));
            skipped.push(input);
        }
        // Each cable's last stretch, into its module, after any groups
        for input in skipped {
            if let Some(hop) = self.graph.connection(input) {
                self.graph.remove_connection(input, hop);
            }
        }
    }

    /// A group's saved jacks. One naming a signal this version doesn't know
    /// carries Control.
    fn jacks(&mut self, group: &str, jacks: &[JackData]) -> Vec<Jack> {
        jacks
            .iter()
            .map(|jack| {
                let signal = signal_named(&jack.signal).unwrap_or_else(|| {
                    self.warnings.push(format!(
                        "Group '{}': jack '{}' carries unknown signal '{}', taken as Control",
                        group, jack.name, jack.signal
                    ));
                    SignalType::Control
                });
                Jack { name: jack.name.clone(), signal }
            })
            .collect()
    }
}

/// Sorts module-to-module cables the way a patch without groups saves them,
/// by module ID and port name, whatever groups they cross. Cables between
/// modules without a patch ID (ones already in the graph) come first.
pub(super) fn patch_order(graph: &SynthGraph, cables: &mut [(OutputId, InputId)], patch_ids: &HashMap<NodeId, u64>) {
    cables.sort_by_cached_key(|&(output_id, input_id)| {
        let (from, to) = (graph.get_output(output_id).node, graph.get_input(input_id).node);
        (
            patch_ids.get(&from).copied(),
            port_name(&graph[from].outputs, output_id),
            patch_ids.get(&to).copied(),
            port_name(&graph[to].inputs, input_id),
        )
    });
}

/// The signal type a jack names.
fn signal_named(name: &str) -> Option<SignalType> {
    [SignalType::Audio, SignalType::Control, SignalType::Gate, SignalType::Midi, SignalType::Bus]
        .into_iter()
        .find(|signal| signal.name().eq_ignore_ascii_case(name))
}

/// Sets a freshly built node's parameters from the patch, by name.
fn restore_parameters(graph: &mut SynthGraph, node_id: NodeId, node_data: &NodeData, warnings: &mut Vec<String>) {
    for saved in &node_data.parameters {
        let input_id = graph.nodes[node_id]
            .inputs
            .iter()
            .find(|(name, id)| *name == saved.name && port_mapping::is_parameter(graph.get_input(*id).kind))
            .map(|(_, id)| *id);
        let Some(input_id) = input_id else {
            warnings.push(format!(
                "{}: ignored unknown parameter '{}'",
                node_data.module_id, saved.name
            ));
            continue;
        };

        // A kind of value a newer Soba saved: the parameter keeps its default
        if let ParameterValue::Unknown { kind, .. } = &saved.value {
            warnings.push(format!(
                "{}: '{}' holds a {} value, which this build doesn't know, so it was left at its default",
                node_data.module_id, saved.name, kind
            ));
            continue;
        }

        // A choice this build doesn't offer (a Clock's Link Source, in a
        // build without Link) stays at its default, rather than becoming
        // whichever choice is last
        let value = &mut graph.inputs[input_id].value;
        if let SynthValueType::Select { options, .. } = value {
            if saved.value.as_f32().round() as usize >= options.len() {
                warnings.push(format!(
                    "{}: '{}' choice {} isn't in this build, so it was left at its default",
                    node_data.module_id,
                    saved.name,
                    saved.value.as_f32().round()
                ));
                continue;
            }
        }

        // Clamped to the parameter's current range
        value.set_actual_value(saved.value.as_f32());
    }
}

/// Index of the parameter called `name` on a node.
fn parameter_index(graph: &SynthGraph, node_id: NodeId, name: &str) -> Option<usize> {
    port_mapping::parameter_inputs(graph, node_id)
        .into_iter()
        .position(|input_id| graph.nodes[node_id].inputs.iter().any(|(n, id)| *id == input_id && n == name))
}

/// The saved form of an editor value.
fn parameter_value(value: &SynthValueType) -> ParameterValue {
    match value {
        SynthValueType::Port | SynthValueType::Number { .. } => ParameterValue::Number(value.actual_value()),
        SynthValueType::Toggle { value, .. } => ParameterValue::Toggle(*value),
        SynthValueType::Select { value, .. } => ParameterValue::Select(*value),
    }
}

/// Name of the port with the given ID.
fn port_name<Id: PartialEq>(ports: &[(String, Id)], wanted: Id) -> Option<String> {
    ports.iter().find(|(_, id)| *id == wanted).map(|(name, _)| name.clone())
}

/// One level of a patch, captured: modules, cables and groups.
#[derive(Debug, Default)]
pub struct CapturedLevel {
    pub nodes: Vec<NodeData>,
    pub connections: Vec<ConnectionData>,
    pub groups: Vec<GroupData>,
}

/// Captures a graph as a patch.
///
/// Modules are saved under their engine IDs (`engine_id`), and groups under
/// their group IDs; modules without an engine ID are left out. `position`
/// gives each node's canonical editor position. Everything is sorted so
/// that saving the same graph twice gives the same file. The patch is
/// marked with the oldest version that can read it.
pub fn capture_patch(
    name: &str,
    graph: &SynthGraph,
    engine_id: impl Fn(NodeId) -> Option<EngineNodeId>,
    position: impl Fn(NodeId) -> (f32, f32),
    midi_mappings: &[MidiMapping],
) -> Patch {
    let mut patch = Patch::new(name);
    let top: Vec<NodeId> = groups::level_nodes(graph, None);
    let level = capture_level(graph, &top, None, &engine_id, &position);
    (patch.nodes, patch.connections, patch.groups) = (level.nodes, level.connections, level.groups);

    // Mappings to deleted nodes would only come back as load warnings
    let saved: HashSet<u64> = patch.all_nodes().into_iter().map(|n| n.id).collect();
    patch.midi_mappings = midi_mappings.iter().filter(|m| saved.contains(&m.node_id)).cloned().collect();
    patch.version = patch.required_version();
    patch
}

/// Captures some nodes on one level, the cables between them, and
/// everything inside any groups among them. With `inside`, the level is
/// that group's inside, and cables through its jacks are kept too, to and
/// from the group's own ID. A group's Inputs and Outputs among `members`
/// are left out: they only go with their group.
pub fn capture_level(
    graph: &SynthGraph,
    members: &[NodeId],
    inside: Option<GroupId>,
    engine_id: &dyn Fn(NodeId) -> Option<EngineNodeId>,
    position: &dyn Fn(NodeId) -> (f32, f32),
) -> CapturedLevel {
    let index = GroupIndex::of(graph);
    let mut level = CapturedLevel::default();
    // The ID each captured node is saved under, as a source and as a destination
    let mut sources: HashMap<NodeId, u64> = HashMap::new();
    let mut targets: HashMap<NodeId, u64> = HashMap::new();
    if let Some(id) = inside {
        let parts = index.parts(id);
        sources.extend(parts.inputs.map(|n| (n, id.0)));
        targets.extend(parts.outputs.map(|n| (n, id.0)));
    }

    for &node_id in members {
        let Some(node) = graph.nodes.get(node_id) else { continue };
        let data = &node.user_data;
        match data.kind {
            NodeKind::Module => {
                let Some(id) = engine_id(node_id) else { continue };
                let mut node_data = NodeData::new(id, data.module_id, position(node_id));
                node_data.bypassed = data.bypassed;
                node_data.pinned = data.pins.clone();
                node_data.face_labels = data.face_labels.clone();
                node_data.file = data.file.clone();
                node_data.labels = data.labels.clone();
                node_data.slice_markers = data.slice_markers.clone();
                node_data.scenes = data.scenes.clone();
                node_data.parameters = node
                    .inputs
                    .iter()
                    .filter(|(_, input_id)| port_mapping::is_parameter(graph.get_input(*input_id).kind))
                    .map(|(name, input_id)| {
                        NamedParameter::new(name.clone(), parameter_value(&graph.get_input(*input_id).value))
                    })
                    .collect();
                level.nodes.push(node_data);
                sources.insert(node_id, id);
                targets.insert(node_id, id);
            }
            NodeKind::Group(id) => {
                let parts = index.parts(id);
                let inner: Vec<NodeId> = groups::level_nodes(graph, Some(id));
                let captured = capture_level(graph, &inner, Some(id), engine_id, position);
                let jacks = |jacks: Vec<Jack>| {
                    jacks.into_iter().map(|j| JackData { name: j.name, signal: j.signal.name().to_string() }).collect()
                };
                level.groups.push(GroupData {
                    id: id.0,
                    name: data.display_name.clone(),
                    position: position(node_id),
                    inputs: jacks(groups::input_jacks(graph, node_id)),
                    outputs: jacks(groups::output_jacks(graph, node_id)),
                    inputs_position: parts.inputs.map(position).unwrap_or_default(),
                    outputs_position: parts.outputs.map(position).unwrap_or_default(),
                    nodes: captured.nodes,
                    connections: captured.connections,
                    groups: captured.groups,
                });
                sources.insert(node_id, id.0);
                targets.insert(node_id, id.0);
            }
            NodeKind::Inputs(_) | NodeKind::Outputs(_) => {}
        }
    }
    level.nodes.sort_by_key(|n| n.id);
    level.groups.sort_by_key(|g| g.id);

    for (input_id, output_id) in graph.iter_connections() {
        let from = graph.get_output(output_id).node;
        let to = graph.get_input(input_id).node;
        let (Some(&from_id), Some(&to_id)) = (sources.get(&from), targets.get(&to)) else {
            continue;
        };
        let (Some(from_port), Some(to_port)) = (
            port_name(&graph.nodes[from].outputs, output_id),
            port_name(&graph.nodes[to].inputs, input_id),
        ) else {
            continue;
        };
        level.connections.push(ConnectionData::new(from_id, from_port, to_id, to_port));
    }
    level.connections.sort_by(|a, b| {
        (a.from_node, &a.from_port, a.to_node, &a.to_port).cmp(&(b.from_node, &b.from_port, b.to_node, &b.to_port))
    });
    level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::patch_from_json;

    const V2_FIXTURE: &str = include_str!("fixtures/patch_v2.json");

    /// Loads a patch the way the editor does (groups and then modules given
    /// IDs from one count starting at `first_engine_id`, modules in node
    /// order, MIDI mappings remapped), then saves it.
    pub(super) fn reload(patch: &Patch, first_engine_id: EngineNodeId) -> (Patch, Vec<String>) {
        let mut staged = stage_patch(patch).unwrap();
        let mut next = first_engine_id;
        renumber_groups(&mut staged.graph, || {
            next += 1;
            GroupId(next - 1)
        });
        let engine_ids: HashMap<NodeId, EngineNodeId> =
            staged.nodes.iter().zip(next..).map(|(n, id)| (n.graph_id, id)).collect();
        let engine_id = |graph_id| engine_ids.get(&graph_id).copied();
        remap_scenes(&mut staged.graph, &staged.nodes, engine_id);
        let mappings = staged.remap_midi_mappings(engine_id);
        let position = |graph_id| {
            let module = staged.nodes.iter().find(|n| n.graph_id == graph_id).map(|n| n.position);
            module.or_else(|| staged.parts.iter().find(|p| p.graph_id == graph_id).map(|p| p.position)).unwrap()
        };
        let saved = capture_patch(&patch.name, &staged.graph, engine_id, position, &mappings);
        (saved, staged.warnings)
    }

    fn node<'a>(patch: &'a Patch, module_id: &str) -> &'a NodeData {
        patch.nodes.iter().find(|n| n.module_id == module_id).unwrap()
    }

    fn param(patch: &Patch, module_id: &str, name: &str) -> f32 {
        node(patch, module_id).parameters.iter().find(|p| p.name == name).unwrap().value.as_f32()
    }

    #[test]
    fn test_a_parameter_type_from_a_newer_soba_keeps_the_rest_of_the_patch() {
        // A v6 patch whose second parameter is of a kind this build has never seen
        let json = r#"{
            "name": "from the future", "version": 6, "connections": [],
            "nodes": [{"id": 1, "module_id": "util.clock", "position": [0.0, 0.0], "parameters": [
                {"name": "Tempo", "type": "Number", "value": 97.0},
                {"name": "Division", "type": "Fraction", "value": [3, 16]}
            ]}]
        }"#;
        let patch = patch_from_json(json).unwrap();

        let (saved, warnings) = reload(&patch, 0);
        assert_eq!(param(&saved, "util.clock", "Tempo"), 97.0);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(warnings[0].contains("Division") && warnings[0].contains("Fraction"), "{:?}", warnings);
    }

    #[test]
    fn test_a_link_clock_keeps_its_own_time_in_a_build_without_link() {
        use crate::persistence::{NamedParameter, ParameterValue};

        let mut patch = Patch::new("linked");
        let mut clock = NodeData::new(1, "util.clock", (0.0, 0.0));
        clock.parameters = vec![NamedParameter::new("Source", ParameterValue::Select(2))];
        patch.nodes.push(clock);

        let (saved, warnings) = reload(&patch, 0);
        if cfg!(feature = "ableton-link") {
            assert!(warnings.is_empty(), "{:?}", warnings);
            assert_eq!(param(&saved, "util.clock", "Source"), 2.0);
        } else {
            // Internal, not MIDI (the last choice), which would wait silent
            // for a clock that never comes. And the load says so
            assert_eq!(param(&saved, "util.clock", "Source"), 0.0);
            assert_eq!(warnings.len(), 1, "{:?}", warnings);
            assert!(warnings[0].contains("Source"), "{:?}", warnings);
        }
    }

    #[test]
    fn test_an_arrangers_names_survive_a_round_trip() {
        let mut patch = Patch::new("arranged");
        let mut arranger = NodeData::new(1, "seq.arranger", (0.0, 0.0));
        arranger.labels.insert("Section 1".into(), "Intro".into());
        arranger.labels.insert("Lane 3".into(), "Pad".into());
        patch.nodes.extend([arranger, NodeData::new(2, "osc.sine", (0.0, 0.0))]);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        let labels = &node(&saved, "seq.arranger").labels;
        assert_eq!(labels.get("Section 1").map(String::as_str), Some("Intro"));
        assert_eq!(labels.get("Lane 3").map(String::as_str), Some("Pad"));
        // Only modules with names mention them
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json.matches("\"labels\"").count(), 1);
        assert_eq!(patch_from_json(&json).unwrap(), saved);
    }

    #[test]
    fn test_scenes_survive_a_round_trip_under_new_ids() {
        use std::collections::BTreeMap;

        let mut patch = Patch::new("scened");
        let mut scenes = NodeData::new(5, "util.scenes", (0.0, 0.0));
        let cutoff = |hz: f32| BTreeMap::from([("Cutoff".to_string(), hz)]);
        scenes.scenes.insert("A".into(), BTreeMap::from([(7, cutoff(300.0)), (99, cutoff(1.0))]));
        scenes.scenes.insert("C".into(), BTreeMap::from([(7, cutoff(4000.0))]));
        patch.nodes.extend([NodeData::new(7, "filter.svf", (0.0, 0.0)), scenes]);

        // The filter loads as 40, the Scenes as 41
        let (saved, warnings) = reload(&patch, 40);
        assert!(warnings.is_empty(), "{:?}", warnings);
        let bank = &node(&saved, "util.scenes").scenes;
        assert_eq!(bank.keys().collect::<Vec<_>>(), ["A", "C"]);
        // ...naming the filter by its new ID, and letting go of a module that isn't there
        assert_eq!(bank["A"].keys().collect::<Vec<_>>(), [&40]);
        assert_eq!(bank["C"][&40]["Cutoff"], 4000.0);
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json.matches("\"scenes\"").count(), 1);
        assert_eq!(patch_from_json(&json).unwrap(), saved);
    }

    #[test]
    fn test_slice_markers_placed_by_hand_survive_a_round_trip() {
        use crate::dsp::onsets::HandMarkers;

        let mut patch = Patch::new("chopped");
        let mut sampler = NodeData::new(1, "source.sampler", (0.0, 0.0));
        sampler.slice_markers = Some(HandMarkers::new(0.65, vec![0.5, 0.125, 0.3125]));
        patch.nodes.extend([sampler, NodeData::new(2, "source.sampler", (0.0, 0.0))]);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        let markers = saved.nodes.iter().find_map(|n| n.slice_markers.clone()).expect("kept");
        assert_eq!(markers.places, [0.125, 0.3125, 0.5]);
        assert_eq!(markers.sensitivity, 0.65);
        // Only a Sampler with markers moved mentions them
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json.matches("\"slice_markers\"").count(), 1);
        assert_eq!(patch_from_json(&json).unwrap(), saved);
    }

    #[test]
    fn test_a_samplers_file_survives_a_round_trip() {
        let mut patch = Patch::new("sampled");
        let mut sampler = NodeData::new(1, "source.sampler", (0.0, 0.0));
        sampler.file = Some("samples/choir ah.wav".to_string());
        patch.nodes.extend([sampler, NodeData::new(2, "osc.sine", (0.0, 0.0))]);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert_eq!(node(&saved, "source.sampler").file.as_deref(), Some("samples/choir ah.wav"));
        assert_eq!(node(&saved, "osc.sine").file, None);
        // Only modules with a file mention it
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json.matches("\"file\"").count(), 1);
        assert_eq!(patch_from_json(&json).unwrap(), saved);
    }

    #[test]
    fn test_bypass_survives_a_round_trip() {
        let mut patch = Patch::new("bypass");
        let mut delay = NodeData::new(1, "fx.delay", (0.0, 0.0));
        delay.bypassed = true;
        // An oscillator has nothing to pass through, so it can't be bypassed
        let mut osc = NodeData::new(2, "osc.sine", (0.0, 0.0));
        osc.bypassed = true;
        patch.nodes.extend([delay, osc, NodeData::new(3, "fx.reverb", (0.0, 0.0))]);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert!(node(&saved, "fx.delay").bypassed);
        assert!(!node(&saved, "osc.sine").bypassed);
        assert!(!node(&saved, "fx.reverb").bypassed);

        // Only bypassed nodes mention it in the file
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(json.matches("bypassed").count(), 1);
        assert_eq!(patch_from_json(&json).unwrap(), saved);
    }

    #[test]
    fn test_a_jack_added_renamed_and_moved_survives_a_round_trip() {
        use crate::graph::groups::{self, Side};
        let patch = patch_from_json(include_str!("../../library/acid-bass.json")).unwrap();
        let mut staged = stage_patch(&patch).unwrap();
        let ladder = staged.nodes.iter().find(|n| n.template.module_id() == "filter.ladder").unwrap();
        let (ladder_graph, ladder_id) = (ladder.graph_id, ladder.patch_id);
        groups::show_port(&mut staged.graph, ladder_graph, Side::Input, "Resonance").unwrap();
        // Renamed and moved up a place, it's saved that way
        let group_node = staged.graph.nodes.iter().find(|(_, n)| n.user_data.kind.group().is_some()).unwrap().0;
        groups::rename_jack(&mut staged.graph, group_node, Side::Input, "Resonance", "Res").unwrap();
        groups::move_jack(&mut staged.graph, group_node, Side::Input, "Res", true).unwrap();

        let ids: HashMap<NodeId, u64> = staged.nodes.iter().map(|n| (n.graph_id, n.patch_id)).collect();
        let saved = capture_patch(&patch.name, &staged.graph, |g| ids.get(&g).copied(), |_| (0.0, 0.0), &[]);
        let json = serde_json::to_string(&saved).unwrap();
        let group = &patch_from_json(&json).unwrap().groups[0];
        let jacks: Vec<&str> = group.inputs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(jacks, ["Pitch", "Gate", "Res", "Velocity"]);
        assert!(group.connections.iter().any(|c| c.from_node == group.id
            && c.from_port == "Res"
            && c.to_node == ladder_id
            && c.to_port == "Resonance"));
    }

    #[test]
    fn test_face_names_survive_a_round_trip_two_groups_up() {
        use crate::graph::GroupId;
        use crate::persistence::GroupData;
        // The Electric Piano inside a Rig, its tine raised to the Rig's face
        let mut piano = patch_from_json(include_str!("../../library/electric-piano.json")).unwrap();
        let mut inner = piano.groups.remove(0);
        let osc = inner.nodes.iter_mut().find(|n| n.face_labels.contains_key("FM Depth")).unwrap();
        assert_eq!(osc.face_labels["FM Depth"], "Tine");
        osc.pinned.insert("FM Depth".into(), 2);
        let mut patch = Patch::new("rig");
        patch.groups.push(GroupData {
            id: 9000,
            name: "Rig".into(),
            position: (0.0, 0.0),
            inputs: Vec::new(),
            outputs: Vec::new(),
            inputs_position: (-320.0, 0.0),
            outputs_position: (640.0, 0.0),
            nodes: Vec::new(),
            connections: Vec::new(),
            groups: vec![inner],
        });

        // Both faces show it, under its name
        let staged = stage_patch(&patch).unwrap();
        let graph = &staged.graph;
        let index = GroupIndex::of(graph);
        let ids: Vec<GroupId> = graph.nodes.values().filter_map(|n| n.user_data.kind.group()).collect();
        assert_eq!(ids.len(), 2);
        for id in ids {
            let faces = groups::face_controls(graph, |_| None, &index, id);
            let tine = faces.iter().find(|c| c.param == "FM Depth").unwrap();
            assert_eq!(graph[tine.node].user_data.face_label("FM Depth"), Some("Tine"));
        }

        let engine_ids: HashMap<NodeId, u64> = staged.nodes.iter().map(|n| (n.graph_id, n.patch_id)).collect();
        let saved = capture_patch(&patch.name, graph, |g| engine_ids.get(&g).copied(), |_| (0.0, 0.0), &[]);
        let json = serde_json::to_string(&saved).unwrap();
        let reloaded = patch_from_json(&json).unwrap();
        assert_eq!(reloaded, saved);
        let osc = reloaded.groups[0].groups[0].nodes.iter().find(|n| n.pinned.get("FM Depth") == Some(&2)).unwrap();
        assert_eq!(osc.face_labels["FM Depth"], "Tine");
        // Only modules named on a face mention it
        assert_eq!(json.matches("\"face_labels\"").count(), 4);
    }

    #[test]
    fn test_quantizer_custom_scale_survives_a_round_trip() {
        use crate::modules::quantizer::CUSTOM_SCALE;
        use crate::persistence::{NamedParameter, ParameterValue};

        // Root, fourth, fifth and flat seventh, in A
        let mask = 0b0100_1010_0001;
        let mut patch = Patch::new("custom scale");
        let mut quantizer = NodeData::new(1, "util.quantizer", (0.0, 0.0));
        quantizer.parameters = vec![
            NamedParameter::new("Root", ParameterValue::Select(9)),
            NamedParameter::new("Scale", ParameterValue::Select(CUSTOM_SCALE)),
            NamedParameter::new("Transpose", ParameterValue::Number(-5.0)),
            NamedParameter::new("Mask", ParameterValue::Number(mask as f32)),
        ];
        patch.nodes.push(quantizer);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        let json = serde_json::to_string(&saved).unwrap();
        let loaded = patch_from_json(&json).unwrap();
        assert_eq!(param(&loaded, "util.quantizer", "Mask"), mask as f32);
        assert_eq!(param(&loaded, "util.quantizer", "Scale"), CUSTOM_SCALE as f32);
        assert_eq!(param(&loaded, "util.quantizer", "Root"), 9.0);
        assert_eq!(param(&loaded, "util.quantizer", "Transpose"), -5.0);
    }

    #[test]
    fn test_trigger_patterns_survive_a_round_trip() {
        use crate::modules::trigger_sequencer::Step;
        use crate::persistence::{NamedParameter, ParameterValue};

        // A beat in A, a ratcheted fill in D, an accent, a lane of its own
        // length and the chain A A A D
        let roll = Step { on: true, velocity: 63, probability: 75, ratchet: 3 };
        let off_but_kept = Step { on: false, velocity: 12, probability: 40, ratchet: 4 };
        let mut patch = Patch::new("trigger");
        let mut seq = NodeData::new(1, "seq.trigger", (0.0, 0.0));
        seq.parameters = vec![
            NamedParameter::new("Step A1 01", ParameterValue::Number(Step { on: true, ..Step::DEFAULT }.encode())),
            NamedParameter::new("Step D8 16", ParameterValue::Number(roll.encode())),
            NamedParameter::new("Step C4 09", ParameterValue::Number(off_but_kept.encode())),
            NamedParameter::new("Accent B 07", ParameterValue::Toggle(true)),
            NamedParameter::new("Length 6", ParameterValue::Number(5.0)),
            NamedParameter::new("Chain 4", ParameterValue::Select(4)),
            NamedParameter::new("Chain 2", ParameterValue::Select(1)),
            NamedParameter::new("Chain 3", ParameterValue::Select(1)),
        ];
        patch.nodes.push(seq);

        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        let json = serde_json::to_string(&saved).unwrap();
        let loaded = patch_from_json(&json).unwrap();
        let step = |name| Step::decode(param(&loaded, "seq.trigger", name));
        assert_eq!(step("Step A1 01"), Step { on: true, ..Step::DEFAULT });
        assert_eq!(step("Step D8 16"), roll);
        assert_eq!(step("Step C4 09"), off_but_kept);
        assert_eq!(step("Step B2 02"), Step::DEFAULT);
        assert_eq!(param(&loaded, "seq.trigger", "Accent B 07"), 1.0);
        assert_eq!(param(&loaded, "seq.trigger", "Accent A 07"), 0.0);
        assert_eq!(param(&loaded, "seq.trigger", "Length 6"), 5.0);
        let chain: Vec<f32> = (1..=4).map(|slot| param(&loaded, "seq.trigger", &format!("Chain {slot}"))).collect();
        assert_eq!(chain, [1.0, 1.0, 1.0, 4.0]);
        // Saved again, it's the same file
        assert_eq!(reload(&loaded, 0).0, loaded);
    }

    #[test]
    fn test_round_trip_keeps_graph_params_and_midi_mappings() {
        let (first, warnings) = reload(&patch_from_json(V2_FIXTURE).unwrap(), 0);
        assert!(warnings.is_empty(), "{:?}", warnings);

        // Load it again under different engine IDs, as a second load in the
        // same session would
        let (second, warnings) = reload(&first, 100);
        assert!(warnings.is_empty(), "{:?}", warnings);

        let mut expected = first.clone();
        for n in &mut expected.nodes {
            n.id += 100;
        }
        for c in &mut expected.connections {
            c.from_node += 100;
            c.to_node += 100;
        }
        for m in &mut expected.midi_mappings {
            m.node_id += 100;
        }
        assert_eq!(second, expected);

        // And through JSON
        let json = serde_json::to_string_pretty(&second).unwrap();
        assert_eq!(patch_from_json(&json).unwrap(), second);
    }

    #[test]
    fn test_v2_fixture_loads() {
        let patch = patch_from_json(V2_FIXTURE).unwrap();
        assert_eq!(patch.version, patch.required_version());
        let (saved, warnings) = reload(&patch, 40);
        assert!(warnings.is_empty(), "{:?}", warnings);

        assert_eq!(saved.nodes.len(), 4);
        assert_eq!(saved.connections.len(), 3);

        // 110 Hz before v5: A2, two octaves and nine semitones from C4
        assert_eq!(param(&saved, "osc.sine", "Octave"), -2.0);
        assert_eq!(param(&saved, "osc.sine", "Semitone"), 9.0);
        assert!(param(&saved, "osc.sine", "Fine").abs() < 0.01);
        assert_eq!(param(&saved, "osc.sine", "Waveform"), 1.0);
        assert_eq!(param(&saved, "osc.sine", "Pulse Width"), 0.3);
        assert_eq!(param(&saved, "filter.svf", "Cutoff"), 640.0);
        assert_eq!(param(&saved, "filter.svf", "Resonance"), 0.8);
        // Saved as 0-1 before v4; now in real units (1-10x)
        assert_eq!(param(&saved, "filter.svf", "Drive"), 3.25);
        assert_eq!(param(&saved, "mod.adsr", "Sustain"), 0.2);
        assert_eq!(param(&saved, "mod.adsr", "Release"), 0.4);
        assert_eq!(param(&saved, "output.audio", "Volume"), 0.6);
        assert_eq!(param(&saved, "output.audio", "Limiter"), 1.0);
        // Saved before Character existed, so it keeps its default
        assert_eq!(param(&saved, "output.audio", "Character"), 0.0);

        // The mappings follow the filter to its new engine ID
        let filter_id = node(&saved, "filter.svf").id;
        assert_eq!(filter_id, 41);
        let targets: Vec<_> = saved.midi_mappings.iter().map(|m| (m.cc_number, m.node_id, m.param_index)).collect();
        assert_eq!(targets, vec![(74, filter_id, 0), (71, filter_id, 1)]);
    }

    #[test]
    fn test_unknown_module_keeps_the_rest_of_the_patch() {
        let mut patch = patch_from_json(V2_FIXTURE).unwrap();
        patch.nodes.push(NodeData::new(20, "fx.from_the_future", (0.0, 0.0)));
        patch.connections.push(ConnectionData::new(20, "Out", 15, "Left"));
        patch.midi_mappings.push(MidiMapping::new(1, 0, 20, 0, "Wobble", 0.0, 1.0));

        let staged = stage_patch(&patch).unwrap();
        assert_eq!(staged.nodes.len(), 4);
        assert_eq!(staged.graph.iter_connections().count(), 3);
        assert!(staged.graph_id(20).is_none());
        assert_eq!(staged.warnings.len(), 3, "{:?}", staged.warnings);
        assert!(staged.warnings[0].contains("fx.from_the_future"));

        let mappings = staged.remap_midi_mappings(|_| Some(0));
        assert_eq!(mappings.len(), 2);
    }

    #[test]
    fn test_audio_input_channel_defaults_to_stereo_and_round_trips() {
        use crate::persistence::{NamedParameter, ParameterValue};

        // Saved before Audio Input had a Channel
        let mut patch = Patch::new("old input");
        let mut input = NodeData::new(1, "source.audio_input", (0.0, 0.0));
        input.parameters = vec![
            NamedParameter::new("Gain", ParameterValue::Number(6.0)),
            NamedParameter::new("Threshold", ParameterValue::Number(-24.0)),
        ];
        patch.nodes.push(input);
        let (saved, warnings) = reload(&patch, 0);
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert_eq!(param(&saved, "source.audio_input", "Channel"), 0.0, "Stereo, as before");
        assert_eq!(param(&saved, "source.audio_input", "Gain"), 6.0);

        // Set to input 1, then saved and loaded again
        let mut changed = saved.clone();
        let channel = changed.nodes[0].parameters.iter_mut().find(|p| p.name == "Channel").unwrap();
        channel.value = ParameterValue::Select(1);
        let json = serde_json::to_string(&reload(&changed, 0).0).unwrap();
        let loaded = patch_from_json(&json).unwrap();
        assert_eq!(param(&loaded, "source.audio_input", "Channel"), 1.0);
        assert_eq!(param(&loaded, "source.audio_input", "Threshold"), -24.0);
    }

    #[test]
    fn test_parameters_restore_by_name_not_position() {
        let mut patch = Patch::new("reordered");
        let mut filter = NodeData::new(1, "filter.svf", (0.0, 0.0));
        filter.parameters = vec![
            NamedParameter::new("Drive", ParameterValue::Number(4.5)),
            NamedParameter::new("Retired Knob", ParameterValue::Scalar(0.1)),
            NamedParameter::new("Cutoff", ParameterValue::Frequency(300.0)),
        ];
        patch.nodes.push(filter);

        let (saved, warnings) = reload(&patch, 0);
        assert_eq!(param(&saved, "filter.svf", "Cutoff"), 300.0);
        assert_eq!(param(&saved, "filter.svf", "Drive"), 4.5);
        // Missing from the patch, so it keeps its default
        assert_eq!(param(&saved, "filter.svf", "Resonance"), 0.5);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Retired Knob"));
    }

    #[test]
    fn test_midi_mapping_follows_parameter_name() {
        // A stale index must not win over the name
        let mut patch = patch_from_json(V2_FIXTURE).unwrap();
        patch.midi_mappings[1].param_index = 2;
        patch.midi_mappings.push(MidiMapping::new(5, 0, 7, 0, "Retired Knob", 0.0, 1.0));

        let staged = stage_patch(&patch).unwrap();
        let mappings = staged.remap_midi_mappings(|_| Some(0));
        let indices: Vec<_> = mappings.iter().map(|m| (m.cc_number, m.param_index)).collect();
        assert_eq!(indices, vec![(74, 0), (71, 1)]);
        assert_eq!(staged.warnings.len(), 1, "{:?}", staged.warnings);
    }

    #[test]
    fn test_a_scene_pad_mapping_loads_though_it_names_no_parameter() {
        let mut patch = patch_from_json(V2_FIXTURE).unwrap();
        let pad = MidiMapping::new(48, 10, 7, 0, "Scene A", 0.0, 1.0).note().recalling(0);
        patch.midi_mappings.push(pad.clone());

        let staged = stage_patch(&patch).unwrap();
        let mappings = staged.remap_midi_mappings(|_| Some(0));
        assert!(mappings.iter().any(|m| m.scene == Some(0) && m.cc_number == 48), "{mappings:?}");
        assert!(staged.warnings.is_empty(), "{:?}", staged.warnings);
    }

    #[test]
    fn test_capture_drops_mappings_to_deleted_nodes() {
        let mut patch = patch_from_json(V2_FIXTURE).unwrap();
        patch.midi_mappings.push(MidiMapping::new(9, 0, 999, 0, "Cutoff", 0.0, 1.0));
        // Bypass staging so the stale mapping reaches capture, as it would
        // after the node was deleted in the editor
        let staged = stage_patch(&patch).unwrap();
        let ids: HashMap<NodeId, u64> = staged.nodes.iter().map(|n| (n.graph_id, n.patch_id)).collect();
        let saved = capture_patch("x", &staged.graph, |g| ids.get(&g).copied(), |_| (0.0, 0.0), &patch.midi_mappings);
        assert_eq!(saved.midi_mappings.len(), 2);
    }
}

#[cfg(test)]
#[path = "format_fixtures.rs"]
mod format_fixtures;
