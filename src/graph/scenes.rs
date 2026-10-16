//! Scenes: where the knobs stood, kept on a Scenes module, and the glide
//! from one scene to another.
//!
//! A Scenes module's scope is the patch, or the group it sits in, however
//! deep. A scene holds every knob and switch in its scope by module (engine
//! ID, which survives undo) and parameter name, leaving out other Scenes
//! modules, the Audio Output, and switches that start or stop things
//! rather than shape the sound.
//!
//! Morphing from one scene to another moves each knob round its own travel,
//! as a hand would turn it, so a frequency glides by octaves rather than
//! rushing through the top of its range. Whole-number knobs, switches and
//! dropdowns hold until halfway, then flip. A knob a cable drives is left to
//! the cable, and a knob only one of the two scenes knows stays put.
//!
//! The app's side (reading Morph, undo, the face's pads) is in
//! `app/synth_app/scenes.rs`.

use std::collections::{BTreeMap, HashMap};

use egui_node_graph2::{InputId, InputParamKind, NodeId};

use crate::engine::NodeId as EngineNodeId;
use crate::modules::scenes::{SCENES_ID, SLOT_LABELS};
use crate::widgets::knob::KnobConfig;

use super::groups::{self, GroupIndex, NodeKind};
use super::{port_mapping, SynthGraph, SynthGraphState, SynthValueType};

/// Scenes a module keeps.
pub const SLOTS: usize = SLOT_LABELS.len();

/// One scene: each knob's value in real units, by module and parameter.
pub type Scene = BTreeMap<EngineNodeId, BTreeMap<String, f32>>;

/// A Scenes module's scenes, by slot letter ("A" to "H"). Empty slots
/// aren't kept.
pub type SceneBank = BTreeMap<String, Scene>;

/// Switches that start, stop or record, or say where sound comes from,
/// rather than shape it: a scene leaves them be.
const NOT_SOUND: &[&str] = &["Channel", "Source", "Run", "Latency", "Take", "Bars", "Rec", "Play"];

/// A slot's letter.
pub fn letter(slot: usize) -> &'static str {
    SLOT_LABELS[slot.min(SLOTS - 1)]
}

/// The modules a Scenes module's scenes hold: every module in its group
/// (or the patch, at the top), however deep, but for other Scenes modules
/// and the Audio Output. In graph order.
pub fn scope(graph: &SynthGraph, scenes: NodeId) -> Vec<NodeId> {
    let Some(node) = graph.nodes.get(scenes) else { return Vec::new() };
    let mut modules = match node.user_data.parent {
        Some(group) => groups::descendants(graph, &GroupIndex::of(graph), group),
        None => graph.nodes.keys().collect(),
    };
    modules.retain(|&n| {
        graph.nodes.get(n).is_some_and(|node| {
            let data = &node.user_data;
            data.kind == NodeKind::Module && data.module_id != SCENES_ID && !data.module_id.starts_with("output.")
        })
    });
    modules.sort();
    modules
}

/// A control a scene holds: its module, its name and its input.
#[derive(Clone, Debug)]
pub struct Control {
    pub module: EngineNodeId,
    pub param: String,
    pub input: InputId,
}

/// The knobs and switches a Scenes module's scenes hold, as they stand,
/// patched or not.
pub fn controls(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId) -> Vec<Control> {
    let mut controls = Vec::new();
    for node_id in scope(graph, scenes) {
        let node = &graph[node_id];
        let Some(module) = user_state.get_engine_node_id(node_id) else { continue };
        // Keyboard and MIDI Note play their first few from live input
        let live: Vec<InputId> = port_mapping::parameter_inputs(graph, node_id)
            .into_iter()
            .take(port_mapping::live_input_parameter_count(node.user_data.module_id))
            .collect();
        for (param, input) in &node.inputs {
            let value = graph.get_input(*input);
            if live.contains(input) || matches!(value.kind, InputParamKind::ConnectionOnly) {
                continue;
            }
            let held = match &value.value {
                SynthValueType::Number { .. } => node.user_data.knob_params.iter().any(|knob| &knob.param_name == param),
                SynthValueType::Toggle { .. } | SynthValueType::Select { .. } => {
                    value.shown_inline && !NOT_SOUND.contains(&param.as_str())
                }
                SynthValueType::Port => false,
            };
            if held {
                controls.push(Control { module, param: param.clone(), input: *input });
            }
        }
    }
    controls
}

/// The knobs as they stand, as a scene.
pub fn capture(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId) -> Scene {
    let mut scene = Scene::new();
    for control in controls(graph, user_state, scenes) {
        let value = graph.get_input(control.input).value.actual_value();
        scene.entry(control.module).or_default().insert(control.param, value);
    }
    scene
}

/// How far round its travel a value sits, 0 to 1.
pub fn travel(value: &SynthValueType, actual: f32) -> f32 {
    match value {
        SynthValueType::Number { spec, .. } => {
            let config = KnobConfig { range: spec.min..=spec.max, logarithmic: spec.logarithmic, ..Default::default() };
            config.travel(actual.clamp(spec.min, spec.max)).clamp(0.0, 1.0)
        }
        _ => {
            let (min, max) = value.range();
            if max > min { ((actual - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 }
        }
    }
}

/// The value `morph` of the way from `from` to `to`, for a control like
/// `value`: round the knob's travel, or a flip at halfway for anything
/// that steps.
pub fn blend(value: &SynthValueType, from: f32, to: f32, morph: f32) -> f32 {
    let morph = morph.clamp(0.0, 1.0);
    match value {
        SynthValueType::Number { spec, .. } if !spec.stepped => {
            let config = KnobConfig { range: spec.min..=spec.max, logarithmic: spec.logarithmic, ..Default::default() };
            let (a, b) = (config.travel(from.clamp(spec.min, spec.max)), config.travel(to.clamp(spec.min, spec.max)));
            config.value_at_travel(a + (b - a) * morph).clamp(spec.min, spec.max)
        }
        _ => {
            if morph < 0.5 { from } else { to }
        }
    }
}

/// The scene's controls that are still in scope, with where they stand
/// and their place in it, each found once.
struct Found<'a> {
    graph: &'a SynthGraph,
    inputs: HashMap<(EngineNodeId, &'a str), InputId>,
}

impl<'a> Found<'a> {
    fn new(graph: &'a SynthGraph, user_state: &SynthGraphState, scenes: NodeId) -> Self {
        // Only what's in scope now: a module moved out of the group is let go
        let in_scope: HashMap<NodeId, ()> = scope(graph, scenes).into_iter().map(|n| (n, ())).collect();
        let mut inputs = HashMap::new();
        for (&node_id, &module) in &user_state.node_id_map {
            if !in_scope.contains_key(&node_id) {
                continue;
            }
            for (param, input) in &graph[node_id].inputs {
                inputs.insert((module, param.as_str()), *input);
            }
        }
        Self { graph, inputs }
    }

    fn input(&self, module: EngineNodeId, param: &str) -> Option<InputId> {
        self.inputs.get(&(module, param)).copied()
    }

    fn patched(&self, input: InputId) -> bool {
        self.graph.connection(input).is_some()
    }
}

/// Where a recall or a morph sets each control it reaches.
pub type Settings = Vec<(InputId, f32)>;

/// What recalling a scene sets: each of its controls still in scope that
/// no cable drives.
pub fn recall(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId, scene: &Scene) -> Settings {
    let found = Found::new(graph, user_state, scenes);
    let mut settings = Settings::new();
    for (&module, params) in scene {
        for (param, &value) in params {
            match found.input(module, param) {
                Some(input) if !found.patched(input) => settings.push((input, value)),
                _ => {}
            }
        }
    }
    settings
}

/// What a morph `morph` of the way from one scene to another sets: each
/// control both scenes hold, still in scope, that no cable drives.
pub fn morph(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId, from: &Scene, to: &Scene, morph: f32) -> Settings {
    let found = Found::new(graph, user_state, scenes);
    let mut settings = Settings::new();
    for (&module, params) in from {
        let Some(arrive) = to.get(&module) else { continue };
        for (param, &a) in params {
            let Some(&b) = arrive.get(param) else { continue };
            let Some(input) = found.input(module, param) else { continue };
            if found.patched(input) {
                continue;
            }
            settings.push((input, blend(&graph.get_input(input).value, a, b, morph)));
        }
    }
    settings
}

/// Sets what a recall or morph says to. Returns the inputs it moved.
pub fn apply(graph: &mut SynthGraph, settings: &Settings) -> Vec<InputId> {
    let mut moved = Vec::new();
    for &(input, value) in settings {
        let Some(param) = graph.inputs.get_mut(input) else { continue };
        let before = param.value.actual_value();
        param.value.set_actual_value(value);
        if param.value.actual_value() != before {
            moved.push(input);
        }
    }
    moved
}

/// Whether every control of a scene still in scope stands where the scene
/// has it (a cable's aside), as after it's stored or recalled.
pub fn matches(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId, scene: &Scene) -> bool {
    let found = Found::new(graph, user_state, scenes);
    let mut any = false;
    for (&module, params) in scene {
        for (param, &value) in params {
            let Some(input) = found.input(module, param) else { continue };
            if found.patched(input) {
                continue;
            }
            any = true;
            let now = graph.get_input(input).value.actual_value();
            if (now - value).abs() > 1e-4 * value.abs().max(1.0) {
                return false;
            }
        }
    }
    any
}

/// How far round its travel each of a scene's knobs sits, 0 to 1, for
/// the pad's little picture of it. Knobs no longer in scope are left out.
pub fn travels(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId, scene: &Scene) -> Vec<f32> {
    let found = Found::new(graph, user_state, scenes);
    scene
        .iter()
        .flat_map(|(&module, params)| params.iter().map(move |(param, &value)| (module, param, value)))
        .filter_map(|(module, param, value)| {
            let input = found.input(module, param)?;
            let control = &graph.get_input(input).value;
            matches!(control, SynthValueType::Number { .. }).then(|| travel(control, value))
        })
        .collect()
}

/// Where a Scenes module's Morph stands: the CV patched into it, held to
/// 0 to 1, or the knob. With its From and To slots.
pub fn morph_at(graph: &SynthGraph, user_state: &SynthGraphState, scenes: NodeId) -> Option<(f32, usize, usize)> {
    let node = graph.nodes.get(scenes)?;
    let value = |name: &str| node.get_input(name).ok().map(|input| (input, graph.get_input(input).value.actual_value()));
    let (morph_input, knob) = value("Morph")?;
    let cv = graph
        .connection(morph_input)
        .and_then(|_| port_mapping::input_port_index(graph, scenes, morph_input))
        .and_then(|port| user_state.get_input_value(user_state.get_engine_node_id(scenes)?, port));
    let slot = |name: &str| value(name).map_or(0, |(_, v)| (v.round().max(0.0) as usize).min(SLOTS - 1));
    Some((cv.unwrap_or(knob).clamp(0.0, 1.0), slot("From"), slot("To")))
}

/// The first slot with no scene, if there is one.
pub fn free_slot(bank: &SceneBank) -> Option<usize> {
    (0..SLOTS).find(|&slot| !bank.contains_key(letter(slot)))
}

/// Points a bank's scenes at other module IDs, as a patch is loaded or
/// pasted and its modules get new ones. Modules `id` doesn't know are let
/// go, and a scene left with none is dropped.
pub fn remap(bank: &mut SceneBank, id: impl Fn(EngineNodeId) -> Option<EngineNodeId>) {
    for scene in bank.values_mut() {
        *scene = std::mem::take(scene).into_iter().filter_map(|(module, params)| Some((id(module)?, params))).collect();
    }
    bank.retain(|_, scene| !scene.is_empty());
}

/// The Scenes module whose scope reaches `node` most closely: one beside
/// it in its own group, else in the group around that, and so on out to
/// the top of the patch.
pub fn scenes_for(graph: &SynthGraph, node: NodeId) -> Option<NodeId> {
    let index = GroupIndex::of(graph);
    let mut level = graph.nodes.get(node)?.user_data.parent;
    // A group's own node reaches the Scenes inside it first
    if let NodeKind::Group(id) = graph[node].user_data.kind {
        level = Some(id);
    }
    loop {
        let mut here: Vec<NodeId> = graph
            .nodes
            .iter()
            .filter(|(_, n)| n.user_data.module_id == SCENES_ID && n.user_data.parent == level)
            .map(|(id, _)| id)
            .collect();
        here.sort();
        if let Some(&first) = here.first() {
            return Some(first);
        }
        let group = level?;
        level = index.parent_of(graph, group);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect};
    use egui_node_graph2::GraphEditorState;
    use crate::app::editing;
    use crate::graph::{GroupId, SynthGraphEditorState, SynthNodeTemplate};

    struct Rig {
        editor: SynthGraphEditorState,
        user_state: SynthGraphState,
    }

    impl Rig {
        fn new() -> Self {
            Self { editor: GraphEditorState::new(1.0), user_state: SynthGraphState::new() }
        }

        fn add(&mut self, module_id: &str) -> NodeId {
            let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
            editing::add_module(&mut self.editor, &mut self.user_state, template, pos2(0.0, 0.0)).0
        }

        fn input(&self, node: NodeId, name: &str) -> InputId {
            self.editor.graph[node].get_input(name).unwrap()
        }

        fn param(&self, node: NodeId, name: &str) -> f32 {
            self.editor.graph.get_input(self.input(node, name)).value.actual_value()
        }

        fn set(&mut self, node: NodeId, name: &str, value: f32) {
            let input = self.input(node, name);
            self.editor.graph.inputs[input].value.set_actual_value(value);
        }

        fn morph(&mut self, scenes: NodeId, from: &Scene, to: &Scene, m: f32) {
            let settings = morph(&self.editor.graph, &self.user_state, scenes, from, to, m);
            apply(&mut self.editor.graph, &settings);
        }
    }

    #[test]
    fn a_scene_holds_the_patch_but_not_the_output_or_itself() {
        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let out = rig.add("output.audio");
        let scenes = rig.add(SCENES_ID);
        let scene = capture(&rig.editor.graph, &rig.user_state, scenes);
        let id = |n| rig.user_state.get_engine_node_id(n).unwrap();
        assert!(scene[&id(filter)].contains_key("Cutoff"));
        assert!(!scene.contains_key(&id(out)));
        assert!(!scene.contains_key(&id(scenes)));
        assert!(matches(&rig.editor.graph, &rig.user_state, scenes, &scene));
    }

    #[test]
    fn a_frequency_glides_by_octaves_and_a_switch_flips_halfway() {
        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let osc = rig.add("osc.sine");
        let scenes = rig.add(SCENES_ID);
        rig.set(filter, "Cutoff", 200.0);
        rig.set(osc, "Waveform", 0.0);
        let a = capture(&rig.editor.graph, &rig.user_state, scenes);
        rig.set(filter, "Cutoff", 3200.0);
        rig.set(osc, "Waveform", 2.0);
        let b = capture(&rig.editor.graph, &rig.user_state, scenes);

        // Halfway between 200 Hz and 3200 Hz, four octaves apart, is two up
        rig.morph(scenes, &a, &b, 0.5);
        assert!((rig.param(filter, "Cutoff") - 800.0).abs() < 1.0, "{}", rig.param(filter, "Cutoff"));
        assert_eq!(rig.param(osc, "Waveform"), 2.0);
        rig.morph(scenes, &a, &b, 0.49);
        assert_eq!(rig.param(osc, "Waveform"), 0.0);
        rig.morph(scenes, &a, &b, 0.0);
        assert!(matches(&rig.editor.graph, &rig.user_state, scenes, &a));
        rig.morph(scenes, &a, &b, 1.0);
        assert!(matches(&rig.editor.graph, &rig.user_state, scenes, &b));
    }

    #[test]
    fn a_patched_knob_is_left_to_its_cable() {
        let mut rig = Rig::new();
        let lfo = rig.add("mod.lfo");
        let filter = rig.add("filter.svf");
        let scenes = rig.add(SCENES_ID);
        rig.set(filter, "Cutoff", 200.0);
        let a = capture(&rig.editor.graph, &rig.user_state, scenes);
        rig.set(filter, "Cutoff", 3200.0);
        let b = capture(&rig.editor.graph, &rig.user_state, scenes);

        let output = rig.editor.graph[lfo].outputs.first().unwrap().1;
        let cutoff = rig.input(filter, "Cutoff");
        rig.editor.graph.add_connection(output, cutoff, 0);
        rig.morph(scenes, &b, &a, 1.0);
        assert_eq!(rig.param(filter, "Cutoff"), 3200.0);
        let recalled = recall(&rig.editor.graph, &rig.user_state, scenes, &a);
        assert!(recalled.iter().all(|(input, _)| *input != cutoff));
    }

    #[test]
    fn scenes_in_a_group_reach_only_the_group() {
        let mut rig = Rig::new();
        let inside = rig.add("filter.svf");
        let scenes = rig.add(SCENES_ID);
        let outside = rig.add("filter.svf");
        let top = rig.add(SCENES_ID);
        let made = groups::group(&mut rig.editor, &[inside, scenes], GroupId(1), "Voice", Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 300.0)))
            .expect("grouped");

        let id = |n| rig.user_state.get_engine_node_id(n).unwrap();
        let held = capture(&rig.editor.graph, &rig.user_state, scenes);
        assert!(held.contains_key(&id(inside)) && !held.contains_key(&id(outside)));
        // ...and the one at the top reaches into the group too
        let all = capture(&rig.editor.graph, &rig.user_state, top);
        assert!(all.contains_key(&id(inside)) && all.contains_key(&id(outside)));

        // A module in the group finds the Scenes beside it, and one outside the top's
        assert_eq!(scenes_for(&rig.editor.graph, inside), Some(scenes));
        assert_eq!(scenes_for(&rig.editor.graph, made.node), Some(scenes));
        assert_eq!(scenes_for(&rig.editor.graph, outside), Some(top));

        // A scene from the top doesn't move the group's knobs from inside
        let outside_id = id(outside);
        rig.set(outside, "Cutoff", 300.0);
        let mut moved = all.clone();
        moved.get_mut(&outside_id).unwrap().insert("Cutoff".into(), 5000.0);
        let settings = recall(&rig.editor.graph, &rig.user_state, scenes, &moved);
        assert!(settings.iter().all(|(input, _)| *input != rig.input(outside, "Cutoff")));
    }

    #[test]
    fn keeping_a_scene_is_an_undo_step_and_a_glide_is_not() {
        use crate::app::undo::History;
        use web_time::Instant;

        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let scenes = rig.add(SCENES_ID);
        let mut history = History::default();
        history.reset(&rig.editor, &rig.user_state);

        let scene = capture(&rig.editor.graph, &rig.user_state, scenes);
        rig.editor.graph[scenes].user_data.scenes.insert("A".into(), scene.clone());
        history.name_next("Keep scene A");
        history.record(&rig.editor, &rig.user_state, false, Instant::now());
        assert_eq!(history.undo_label(), Some("Keep scene A"));

        // A glide's knobs, absorbed as the app does, make no step
        let mut far = scene.clone();
        far.get_mut(&rig.user_state.get_engine_node_id(filter).unwrap()).unwrap().insert("Cutoff".into(), 5000.0);
        let settings = morph(&rig.editor.graph, &rig.user_state, scenes, &scene, &far, 0.5);
        let engine_id = rig.user_state.get_engine_node_id(filter).unwrap();
        for input in apply(&mut rig.editor.graph, &settings) {
            let index = port_mapping::parameter_inputs(&rig.editor.graph, filter).iter().position(|&i| i == input).unwrap();
            history.absorb_param(engine_id, index, rig.editor.graph.get_input(input).value.actual_value());
        }
        history.record(&rig.editor, &rig.user_state, false, Instant::now());
        assert_eq!(history.undo_label(), Some("Keep scene A"));

        history.undo(&mut rig.editor, &mut rig.user_state).expect("a step");
        assert!(rig.editor.graph[scenes].user_data.scenes.is_empty());
        history.redo(&mut rig.editor, &mut rig.user_state).expect("a step");
        assert_eq!(rig.editor.graph[scenes].user_data.scenes.get("A"), Some(&scene));
    }

    #[test]
    fn remapping_follows_new_ids_and_drops_the_rest() {
        let mut bank = SceneBank::new();
        bank.insert("A".into(), Scene::from([(3, BTreeMap::from([("Cutoff".to_string(), 1.0)])), (4, BTreeMap::new())]));
        bank.insert("B".into(), Scene::from([(9, BTreeMap::new())]));
        remap(&mut bank, |id| (id != 9).then_some(id + 100));
        assert_eq!(bank.keys().collect::<Vec<_>>(), ["A"]);
        assert!(bank["A"].contains_key(&103) && bank["A"].contains_key(&104));
        assert_eq!(free_slot(&bank), Some(1));
    }

    #[test]
    fn blend_holds_whole_numbers_until_halfway() {
        let stepped = SynthValueType::Number {
            value: 0.0,
            spec: crate::graph::NumberSpec { min: -2.0, max: 2.0, default: 0.0, unit: "", logarithmic: false, stepped: true },
        };
        assert_eq!(blend(&stepped, -2.0, 2.0, 0.4), -2.0);
        assert_eq!(blend(&stepped, -2.0, 2.0, 0.5), 2.0);
    }
}
