//! Arranger Follow: one Arranger playing another's sections.
//!
//! An Arranger has eight lanes, and a song can need more. Patch one
//! Arranger's **Section** output into another's **Follow** input and the
//! second *follows* the first. Its sections (how many, how long, how many
//! times each plays, what they're called), its Steps and its Loop are kept
//! the same as its leader's, and can't be edited on it. Its lanes and their
//! cues stay its own.
//!
//! It all happens in the editor. Each frame, before the engine hears about
//! the frame's edits, [`sync`] copies every leader's song into its
//! followers, so an edit to a leader and the copies of it are one undo step.
//! The engine plays two Arrangers counting one Clock, which already keep
//! step to the sample, so a saved patch plays the same in a build without
//! Follow. Adding, copying, moving or deleting a leader's section moves its
//! followers' cues along with it (see `arranger_display`), so a cue stays
//! with its section.
//!
//! A follower can lead another, so a chain of three plays 24 lanes. A
//! chain that comes back round is refused when it's patched (see
//! [`refusal`]), and [`sync`] leaves alone any that get in another way.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use egui_node_graph2::{InputId, NodeId, OutputId};

use crate::dsp::DspModule;
use crate::modules::arranger::{Arranger, SECTIONS};

use super::arranger_display::section_key;
use super::{groups, SynthGraph, SynthGraphState};

/// The Arranger's module ID.
pub const MODULE: &str = "seq.arranger";
/// The input a leader's Section is patched into.
pub const FOLLOW: &str = "Follow";
/// The leader's output that's patched.
const SECTION: &str = "Section";
/// The key an Arranger's own name is saved under, with its sections' and
/// lanes' names.
pub const NAME_KEY: &str = "Name";

/// The parameters a follower takes from its leader, by index: Steps,
/// Sections, Loop and every section's length and repeats.
static MIRRORED: LazyLock<Vec<usize>> = LazyLock::new(|| {
    [Arranger::PARAM_STEPS, Arranger::PARAM_SECTIONS, Arranger::PARAM_LOOP]
        .into_iter()
        .chain(Arranger::PARAM_LENGTH..Arranger::PARAM_LENGTH + SECTIONS)
        .chain(Arranger::PARAM_REPEAT..Arranger::PARAM_REPEAT + SECTIONS)
        .collect()
});

/// The names of the Arranger's parameters, in order.
static PARAM_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| Arranger::new().parameters().iter().map(|p| p.name).collect());

fn is_arranger(graph: &SynthGraph, node: NodeId) -> bool {
    graph.nodes.get(node).is_some_and(|n| n.user_data.module_id == MODULE)
}

/// An Arranger's name: the one it was given, or "Arranger".
pub fn name(graph: &SynthGraph, node: NodeId) -> String {
    given_name(graph, node).unwrap_or_else(|| "Arranger".to_string())
}

/// The name an Arranger was given, if it was given one.
pub fn given_name(graph: &SynthGraph, node: NodeId) -> Option<String> {
    graph.nodes.get(node).and_then(|n| n.user_data.labels.get(NAME_KEY).cloned())
}

/// The follower and leader a cable joins, if it's an Arranger's Section
/// into another's Follow.
fn link(graph: &SynthGraph, output: OutputId, input: InputId) -> Option<(NodeId, NodeId)> {
    let follower = graph.inputs.get(input)?.node;
    let leader = graph.outputs.get(output)?.node;
    let follows = is_arranger(graph, follower) && graph[follower].get_input(FOLLOW).ok() == Some(input);
    let leads = is_arranger(graph, leader) && graph[leader].get_output(SECTION).ok() == Some(output);
    (follows && leads).then_some((follower, leader))
}

/// Every Arranger that follows another, and the one it follows: cable by
/// cable from module to module, through any groups between.
pub fn leaders(graph: &SynthGraph) -> HashMap<NodeId, NodeId> {
    groups::leaf_cables(graph).into_iter().filter_map(|(output, input)| link(graph, output, input)).collect()
}

/// Whether following leaders up from `node` comes back to it.
fn in_loop(leaders: &HashMap<NodeId, NodeId>, node: NodeId) -> bool {
    let mut at = node;
    for _ in 0..leaders.len() {
        match leaders.get(&at) {
            Some(&leader) if leader == node => return true,
            Some(&leader) => at = leader,
            None => return false,
        }
    }
    false
}

/// How many leaders are above `node`, up to the top of its chain.
fn depth(leaders: &HashMap<NodeId, NodeId>, node: NodeId) -> usize {
    let mut at = node;
    let mut depth = 0;
    while let Some(&leader) = leaders.get(&at) {
        depth += 1;
        at = leader;
        if depth > leaders.len() {
            break;
        }
    }
    depth
}

/// Every Arranger that follows `leader`, directly or down a chain, nearest
/// first.
pub fn followers(leaders: &HashMap<NodeId, NodeId>, leader: NodeId) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut seen = HashSet::from([leader]);
    let mut next = 0;
    let mut from = leader;
    loop {
        let mut direct: Vec<NodeId> = leaders.iter().filter(|(_, &l)| l == from).map(|(&f, _)| f).collect();
        direct.sort();
        for follower in direct {
            if seen.insert(follower) {
                found.push(follower);
            }
        }
        let Some(&follower) = found.get(next) else { return found };
        from = follower;
        next += 1;
    }
}

/// The Arranger `node` follows, if it follows one, as the editor last saw it.
pub fn leader(user_state: &SynthGraphState, node: NodeId) -> Option<NodeId> {
    user_state.arranger_leaders.get(&node).copied()
}

/// The Arranger at the top of `node`'s chain: the leader no one leads, or
/// `node` itself if it follows no one. A chain that comes back round has
/// no top, so an Arranger that leads into one is its own.
pub fn head(leaders: &HashMap<NodeId, NodeId>, node: NodeId) -> NodeId {
    let mut at = node;
    for _ in 0..=leaders.len() {
        match leaders.get(&at) {
            Some(&leader) => at = leader,
            None => return at,
        }
    }
    node
}

/// A follower's Steps knob, which its leader turns.
pub fn steps_takeover(graph: &SynthGraph, user_state: &SynthGraphState, node: NodeId) -> Option<super::clock_display::KnobTakeover> {
    let leader = leader(user_state, node)?;
    let steps = param_inputs(graph, node).map(|inputs| graph.get_input(inputs[Arranger::PARAM_STEPS]).value.actual_value())?;
    Some(super::clock_display::KnobTakeover { value: None, text: format!("{steps:.0} · {}", name(graph, leader)) })
}

/// Whether a control on a node's face is its leader's, so it can't be
/// changed there: a follower's Loop.
pub fn locks(module_id: &str, param: &str, node: NodeId, user_state: &SynthGraphState) -> bool {
    module_id == MODULE && param == "Loop" && leader(user_state, node).is_some()
}

/// Why a cable just patched can't stay, if it's into an Arranger's Follow
/// and can't: it isn't another Arranger's Section, or it closes a loop.
pub fn refusal(graph: &SynthGraph, output: OutputId, input: InputId) -> Option<String> {
    let follower = graph.inputs.get(input)?.node;
    if !is_arranger(graph, follower) || graph[follower].get_input(FOLLOW).ok() != Some(input) {
        return None;
    }
    let leaders = leaders(graph);
    let Some(&leader) = leaders.get(&follower) else {
        // A group's jack may have nothing behind it yet; a module is wrong
        let from = graph.outputs.get(output)?.node;
        let module = graph.nodes.get(from)?.user_data.is_module();
        return module.then(|| "Follow takes another Arranger's Section output".to_string());
    };
    if leader == follower {
        return Some("An Arranger can't follow itself".to_string());
    }
    in_loop(&leaders, follower).then(|| {
        format!(
            "{} already follows {}, so {} can't follow it back",
            name(graph, leader),
            name(graph, follower),
            name(graph, follower)
        )
    })
}

/// Makes every follower's song its leader's: Steps, Sections, Loop, each
/// section's length, repeats and name. Leaders higher up a chain go first, so a
/// chain settles in one go. Returns whether anything changed.
pub fn sync(graph: &mut SynthGraph, leaders: &HashMap<NodeId, NodeId>) -> bool {
    let mut order: Vec<NodeId> = leaders.keys().copied().filter(|&f| !in_loop(leaders, f)).collect();
    order.sort_by_key(|&f| (depth(leaders, f), f));
    let mut changed = false;
    for follower in order {
        changed |= mirror(graph, leaders[&follower], follower);
    }
    changed
}

/// The input of each of an Arranger's parameters, by index.
fn param_inputs(graph: &SynthGraph, node: NodeId) -> Option<Vec<InputId>> {
    let inputs = &graph.nodes.get(node)?.inputs;
    let first = inputs.iter().position(|(name, _)| name == PARAM_NAMES[0])?;
    let params: Vec<InputId> = inputs[first..].iter().map(|(_, id)| *id).collect();
    (params.len() >= PARAM_NAMES.len()).then_some(params)
}

/// Copies one leader's song into one follower.
fn mirror(graph: &mut SynthGraph, leader: NodeId, follower: NodeId) -> bool {
    let (Some(from), Some(to)) = (param_inputs(graph, leader), param_inputs(graph, follower)) else { return false };
    let mut changed = false;
    for &index in MIRRORED.iter() {
        let value = graph.get_input(from[index]).value.actual_value();
        let input = &mut graph.inputs[to[index]];
        if input.value.actual_value() != value {
            input.value.set_actual_value(value);
            changed = true;
        }
    }
    let names: Vec<(String, Option<String>)> =
        (0..SECTIONS).map(|s| section_key(s)).map(|key| (key.clone(), graph[leader].user_data.labels.get(&key).cloned())).collect();
    let labels = &mut graph[follower].user_data.labels;
    for (key, name) in names {
        if labels.get(&key) != name.as_ref() {
            match name {
                Some(name) => labels.insert(key, name),
                None => labels.remove(&key),
            };
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{create_editor_state, SynthGraphEditorState, SynthNodeTemplate};
    use egui_node_graph2::NodeTemplateTrait;

    struct Rig {
        editor: SynthGraphEditorState,
    }

    impl Rig {
        fn new() -> Self {
            Self { editor: create_editor_state() }
        }

        fn add(&mut self, module_id: &str) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            let mut user_state = SynthGraphState::new();
            self.editor.graph.add_node(
                template.node_graph_label(&mut user_state),
                template.user_data(&mut user_state),
                |graph, node_id| template.build_node(graph, &mut user_state, node_id),
            )
        }

        /// Patches `from`'s Section into `to`'s Follow, and returns why
        /// it's refused, if it is (and takes it out again).
        fn follow(&mut self, to: NodeId, from: NodeId) -> Option<String> {
            self.patch(from, SECTION, to, FOLLOW)
        }

        fn patch(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) -> Option<String> {
            let graph = &mut self.editor.graph;
            let (output, input) = (graph[from].get_output(output).unwrap(), graph[to].get_input(input).unwrap());
            graph.add_connection(output, input, 0);
            let refused = refusal(graph, output, input);
            if refused.is_some() {
                graph.remove_connection(input, output);
            }
            refused
        }

        fn set(&mut self, node: NodeId, index: usize, value: f32) {
            let input = param_inputs(&self.editor.graph, node).unwrap()[index];
            self.editor.graph.inputs[input].value.set_actual_value(value);
        }

        fn get(&self, node: NodeId, index: usize) -> f32 {
            let input = param_inputs(&self.editor.graph, node).unwrap()[index];
            self.editor.graph.get_input(input).value.actual_value()
        }

        fn sync(&mut self) -> bool {
            let leaders = leaders(&self.editor.graph);
            sync(&mut self.editor.graph, &leaders)
        }

        fn name(&mut self, node: NodeId, key: &str, text: &str) {
            self.editor.graph[node].user_data.labels.insert(key.to_string(), text.to_string());
        }

        fn label(&self, node: NodeId, key: &str) -> Option<&str> {
            self.editor.graph[node].user_data.labels.get(key).map(String::as_str)
        }
    }

    #[test]
    fn a_follower_takes_its_leaders_song_and_keeps_its_own_lanes() {
        let mut rig = Rig::new();
        let (desk, cues) = (rig.add(MODULE), rig.add(MODULE));
        rig.set(desk, Arranger::PARAM_SECTIONS, 3.0);
        rig.set(desk, Arranger::PARAM_LENGTH + 1, 8.0);
        rig.set(desk, Arranger::PARAM_LOOP, 2.0);
        rig.set(desk, Arranger::PARAM_STEPS, 12.0);
        rig.name(desk, "Section 2", "Groove");
        rig.name(desk, "Lane 1", "Sine");
        rig.name(desk, NAME_KEY, "Desk");
        rig.set(cues, Arranger::cue_param(1, 0), 1_000_500.0);
        rig.name(cues, "Section 3", "Old");
        rig.name(cues, "Lane 1", "Drums");

        assert_eq!(rig.follow(cues, desk), None);
        assert!(rig.sync());
        for index in [Arranger::PARAM_SECTIONS, Arranger::PARAM_LENGTH + 1, Arranger::PARAM_LOOP, Arranger::PARAM_STEPS] {
            assert_eq!(rig.get(cues, index), rig.get(desk, index));
        }
        assert_eq!(rig.label(cues, "Section 2"), Some("Groove"));
        assert_eq!(rig.label(cues, "Section 3"), None, "a name the leader hasn't goes");
        // Its lanes, their names and its own name stay its own
        assert_eq!(rig.get(cues, Arranger::cue_param(1, 0)), 1_000_500.0);
        assert_eq!(rig.label(cues, "Lane 1"), Some("Drums"));
        assert_eq!(rig.label(cues, NAME_KEY), None);
        assert!(!rig.sync(), "nothing more to do");

        // An edit to the leader reaches it; one to it is put back
        rig.set(desk, Arranger::PARAM_LENGTH, 2.0);
        rig.set(cues, Arranger::PARAM_LENGTH + 1, 3.0);
        assert!(rig.sync());
        assert_eq!(rig.get(cues, Arranger::PARAM_LENGTH), 2.0);
        assert_eq!(rig.get(cues, Arranger::PARAM_LENGTH + 1), 8.0);
    }

    #[test]
    fn unpatched_a_follower_keeps_its_sections() {
        let mut rig = Rig::new();
        let (desk, cues) = (rig.add(MODULE), rig.add(MODULE));
        rig.set(desk, Arranger::PARAM_SECTIONS, 5.0);
        rig.follow(cues, desk);
        rig.sync();
        let (output, input) = (rig.editor.graph[desk].get_output(SECTION).unwrap(), rig.editor.graph[cues].get_input(FOLLOW).unwrap());
        rig.editor.graph.remove_connection(input, output);
        rig.set(desk, Arranger::PARAM_SECTIONS, 2.0);
        assert!(!rig.sync());
        assert_eq!(rig.get(cues, Arranger::PARAM_SECTIONS), 5.0);
    }

    #[test]
    fn a_chain_settles_in_one_go_and_lists_its_followers_nearest_first() {
        let mut rig = Rig::new();
        let (a, b, c) = (rig.add(MODULE), rig.add(MODULE), rig.add(MODULE));
        // Patched bottom up, so the last link is the top one
        assert_eq!(rig.follow(c, b), None);
        assert_eq!(rig.follow(b, a), None);
        rig.set(a, Arranger::PARAM_SECTIONS, 7.0);
        rig.sync();
        assert_eq!(rig.get(c, Arranger::PARAM_SECTIONS), 7.0);
        let leaders = leaders(&rig.editor.graph);
        assert_eq!(followers(&leaders, a), [b, c]);
        assert_eq!(followers(&leaders, b), [c]);
        assert!(followers(&leaders, c).is_empty());
    }

    #[test]
    fn a_loop_is_refused() {
        let mut rig = Rig::new();
        let (a, b, c) = (rig.add(MODULE), rig.add(MODULE), rig.add(MODULE));
        rig.name(a, NAME_KEY, "Desk");
        rig.name(c, NAME_KEY, "Cues");
        assert_eq!(rig.follow(a, a).as_deref(), Some("An Arranger can't follow itself"));
        assert_eq!(rig.follow(b, a), None);
        assert_eq!(rig.follow(c, b), None);
        assert_eq!(rig.follow(a, c).as_deref(), Some("Cues already follows Desk, so Desk can't follow it back"));
        assert!(rig.editor.graph[a].get_input(FOLLOW).is_ok_and(|input| rig.editor.graph.connection(input).is_none()));
    }

    #[test]
    fn follow_takes_only_an_arrangers_section() {
        let mut rig = Rig::new();
        let (a, b, lfo) = (rig.add(MODULE), rig.add(MODULE), rig.add("mod.lfo"));
        let refused = rig.patch(lfo, "Out", a, FOLLOW);
        assert_eq!(refused.as_deref(), Some("Follow takes another Arranger's Section output"));
        assert!(rig.patch(b, "Lane 1", a, FOLLOW).is_some());
        // Other cables into an Arranger are none of its business
        assert_eq!(rig.patch(lfo, "Out", a, "Jump To"), None);
    }

    #[test]
    fn a_loop_patched_some_other_way_is_left_alone() {
        let mut rig = Rig::new();
        let (a, b) = (rig.add(MODULE), rig.add(MODULE));
        for (from, to) in [(a, b), (b, a)] {
            let graph = &mut rig.editor.graph;
            let (output, input) = (graph[from].get_output(SECTION).unwrap(), graph[to].get_input(FOLLOW).unwrap());
            graph.add_connection(output, input, 0);
        }
        rig.set(a, Arranger::PARAM_SECTIONS, 6.0);
        assert!(!rig.sync());
        assert_eq!(rig.get(b, Arranger::PARAM_SECTIONS), 4.0);
    }
}
