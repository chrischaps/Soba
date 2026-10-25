//! Mutate: Ctrl+M nudges the selected modules' knobs a small random step,
//! and pressing it again wanders further. The longer it's held, the further
//! they go, from a nudge to a big jump, gliding out along one direction, so
//! the variation can be heard growing. Letting go keeps it.
//!
//! A variation is its seed and its amount: each knob's step comes from the
//! seed and the knob (its module's engine ID and its name), so the same
//! seed and amount move the same knobs the same way from the same place.
//! Steps are taken round the knob's travel, as a hand would turn it, so a
//! frequency moves as far up an octave as down one, and a step past either
//! end folds back.
//!
//! Some knobs never move: one a cable drives, the Audio Output's, and any
//! that sets how loud something is (a level, gain, makeup, feedback or how
//! hard a note is struck), so no variation is louder than the patch was.
//! Whole-number knobs, switches and dropdowns move only when Alt+Ctrl+M
//! asks for them.
//!
//! An Instrument's or a Kit's knobs move the hardware (#447), each as the
//! parameter it holds allows: never one its profile marks `mutate: false`
//! (a volume, a tuning, a MIDI setting), nor one that nudges the device
//! rather than setting it, which undo couldn't walk back. One holding named
//! values is a whole-number knob: Alt+Ctrl+M moves it by whole steps.
//!
//! This holds what's mutated and how. The app's side (the key, the strip of
//! variations, undo) is in `synth_app/mutating.rs`.

use std::collections::VecDeque;

use egui_node_graph2::{InputId, InputParamKind, NodeId};

use crate::engine::NodeId as EngineNodeId;
use crate::graph::groups::{self, GroupIndex, NodeKind};
use crate::graph::{SynthGraph, SynthGraphState, SynthValueType};
use crate::hardware::{self, instrument, Parameter};
use crate::modules::noise::Pcg32;
use crate::widgets::knob::KnobConfig;

/// How far round its travel a knob moves on a quick press, at most.
pub const NUDGE: f32 = 0.06;
/// ...and after the key has been held for [`RAMP_SECONDS`].
pub const JUMP: f32 = 0.5;
/// How long a hold takes to grow from a nudge to a jump.
pub const RAMP_SECONDS: f32 = 2.5;
/// Variations the strip keeps.
pub const KEPT: usize = 8;

/// Words in a parameter's name that make it set how loud something is.
const LOUDNESS: &[&str] = &["Level", "Volume", "Master", "Gain", "Output", "Makeup", "Feedback", "Drive", "Velocity"];
/// Switches that would stop the patch hearing or keeping time, or start
/// recording, or send it to another device or in another's format,
/// rather than change how it sounds.
const NOT_SOUND: &[&str] = &["Channel", "Port", "Source", "Run", "Latency", "Take", "Bars", "Format"];

/// How far a variation goes after the key has been held this long.
pub fn amount(held_seconds: f32) -> f32 {
    let t = (held_seconds / RAMP_SECONDS).clamp(0.0, 1.0);
    NUDGE + (JUMP - NUDGE) * t * t
}

/// A knob, as a variation names it: by its module's engine ID, which
/// survives undo, and its parameter's name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KnobKey {
    pub module: EngineNodeId,
    pub param: String,
}

impl KnobKey {
    /// This knob's own stream of the seed's randomness.
    fn rng(&self, seed: u64) -> Pcg32 {
        // FNV-1a over the knob, so the stream doesn't hang on the order
        // knobs are found in
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.module.to_le_bytes().iter().chain(self.param.as_bytes()) {
            hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        Pcg32::new(seed ^ hash)
    }
}

/// A knob a mutation can move, and where it was when the mutation began.
#[derive(Clone, Debug)]
pub struct Knob {
    pub key: KnobKey,
    pub from: SynthValueType,
    /// For an Instrument's knob holding a stepped parameter (#447), how
    /// many steps share its travel: it moves by whole ones, as a
    /// whole-number knob does, landing in the middle of one.
    pub steps: Option<usize>,
}

impl Knob {
    /// Where this knob goes in the variation from `seed`, `amount` of the
    /// way round its travel at most.
    pub fn mutated(&self, seed: u64, amount: f32) -> f32 {
        let mut rng = self.key.rng(seed);
        let step = rng.bipolar();
        match &self.from {
            SynthValueType::Number { value, spec } if self.steps.is_some_and(|count| count > 1) => {
                let count = self.steps.unwrap_or(1) as f32;
                let span = (spec.max - spec.min).max(f32::EPSILON);
                let last = count - 1.0;
                let at = (((value - spec.min) / span).clamp(0.0, 1.0) * count).floor().min(last);
                let to = (fold(at / last + step * amount) * last).round();
                if to == at { *value } else { spec.min + span * (to + 0.5) / count }
            }
            SynthValueType::Number { value, spec } => {
                let config = KnobConfig { range: spec.min..=spec.max, logarithmic: spec.logarithmic, ..Default::default() };
                let travel = fold(config.travel(*value) + step * amount);
                let moved = config.value_at_travel(travel).clamp(spec.min, spec.max);
                if spec.stepped { moved.round() } else { moved }
            }
            // A switch flips, and a dropdown picks another choice, about as
            // often as a knob would move a third of the way
            SynthValueType::Toggle { value, .. } => {
                let flips = step.abs() < amount * 1.5;
                if *value != flips { 1.0 } else { 0.0 }
            }
            SynthValueType::Select { value, options, .. } => {
                let others = options.len().saturating_sub(1);
                if others == 0 || step.abs() >= amount * 1.5 {
                    return *value as f32;
                }
                let pick = ((rng.bipolar() * 0.5 + 0.5) * others as f32) as usize;
                ((*value + 1 + pick.min(others - 1)) % options.len()) as f32
            }
            SynthValueType::Port => 0.0,
        }
    }
}

/// Folds a travel back into 0 to 1, as a step past either end bounces off it.
fn fold(travel: f32) -> f32 {
    let t = travel.rem_euclid(2.0);
    if t > 1.0 { 2.0 - t } else { t }
}

/// Whether a parameter sets how loud something is.
fn loudness(param: &str, value: &SynthValueType) -> bool {
    let decibels = matches!(value, SynthValueType::Number { spec, .. } if spec.unit == "dB");
    decibels || LOUDNESS.iter().any(|word| param.contains(word))
}

/// Whether Mutate moves an Instrument's knob holding `parameter` (#447):
/// not one its profile keeps from it, nor one that nudges the device,
/// which undo couldn't walk back, nor a loud one (in any case, as makers
/// write them: an "Overdrive").
pub fn moves(parameter: &Parameter) -> bool {
    let name = parameter.name.to_lowercase();
    parameter.mutate && !parameter.relative && !LOUDNESS.iter().any(|word| name.contains(&word.to_lowercase()))
}

/// The modules a selection reaches: those selected, and every module inside
/// a selected group, however deep. In patch order, each once.
pub fn modules(graph: &SynthGraph, selection: &[NodeId]) -> Vec<NodeId> {
    let index = GroupIndex::of(graph);
    let mut modules = Vec::new();
    for &node_id in selection {
        let Some(node) = graph.nodes.get(node_id) else { continue };
        match node.user_data.kind {
            NodeKind::Module => modules.push(node_id),
            NodeKind::Group(id) => modules.extend(groups::descendants(graph, &index, id)),
            NodeKind::Inputs(_) | NodeKind::Outputs(_) => {}
        }
    }
    modules.retain(|n| graph.nodes.get(*n).is_some_and(|node| node.user_data.kind == NodeKind::Module));
    modules.sort();
    modules.dedup();
    modules
}

/// The knobs a mutation of the selection moves, as they stand. `switches`
/// adds whole-number knobs, switches and dropdowns.
pub fn knobs(graph: &SynthGraph, user_state: &SynthGraphState, selection: &[NodeId], switches: bool) -> Vec<Knob> {
    let patched = |input: InputId| graph.connection(input).is_some();
    let mut profiles = None;
    let mut knobs = Vec::new();
    for node_id in modules(graph, selection) {
        let node = &graph[node_id];
        let data = &node.user_data;
        let Some(module) = user_state.get_engine_node_id(node_id) else { continue };
        if data.module_id.starts_with("output.") {
            continue;
        }
        let mut add = |param: &str, input: InputId, steps: Option<usize>| {
            let value = &graph.get_input(input).value;
            if patched(input) || loudness(param, value) {
                return;
            }
            knobs.push(Knob { key: KnobKey { module, param: param.to_string() }, from: value.clone(), steps });
        };
        let plays = instrument::plays_a_profile(data.module_id);
        for knob in &data.knob_params {
            let Ok(input) = node.get_input(&knob.param_name) else { continue };
            // An Instrument's knob moves as the parameter it holds may (#447)
            if let Some(slot) = instrument::slot_named(&knob.param_name).filter(|_| plays) {
                let profiles = profiles.get_or_insert_with(hardware::known);
                let Some(held) = instrument::held(&data.labels, slot, profiles) else { continue };
                let parameter = &held.parameter;
                if !moves(parameter) {
                    continue;
                }
                let steps = instrument::steps_of(parameter);
                if steps.is_none_or(|count| switches && count > 1) {
                    add(&knob.param_name, input, steps);
                }
                continue;
            }
            match &graph.get_input(input).value {
                SynthValueType::Number { spec, .. } if switches || !spec.stepped => add(&knob.param_name, input, None),
                _ => {}
            }
        }
        if switches {
            // The switches and dropdowns on the module's face, as a group's
            // face shows them
            for (param, input) in &node.inputs {
                let input_param = graph.get_input(*input);
                let switch = matches!(input_param.value, SynthValueType::Toggle { .. } | SynthValueType::Select { .. });
                if switch && input_param.shown_inline && !matches!(input_param.kind, InputParamKind::ConnectionOnly) && !NOT_SOUND.contains(&param.as_str()) {
                    add(param, *input, None);
                }
            }
        }
    }
    knobs
}

/// Where a variation leaves its knobs.
pub type Values = Vec<(KnobKey, f32)>;

/// One variation: what made it, and where it left its knobs.
#[derive(Clone, Debug)]
pub struct Variation {
    pub seed: u64,
    pub amount: f32,
    pub values: Values,
    /// How far round its travel each knob sits (0 to 1), for the chip's
    /// little picture of it.
    pub travels: Vec<f32>,
}

/// The variations of one selection, newest last, and the knobs as they
/// were before the first of them.
#[derive(Clone, Debug)]
pub struct Strip {
    /// What was selected, sorted.
    pub selection: Vec<NodeId>,
    pub original: Variation,
    pub variations: VecDeque<Variation>,
}

impl Strip {
    pub fn new(selection: &[NodeId], original: Variation) -> Self {
        let mut selection = selection.to_vec();
        selection.sort();
        Self { selection, original, variations: VecDeque::new() }
    }

    /// Whether it's the strip for this selection.
    pub fn is_for(&self, selection: &[NodeId]) -> bool {
        let mut selection = selection.to_vec();
        selection.sort();
        self.selection == selection
    }

    /// Keeps a new variation, dropping the oldest past [`KEPT`].
    pub fn keep(&mut self, variation: Variation) {
        if self.variations.len() == KEPT {
            self.variations.pop_front();
        }
        self.variations.push_back(variation);
    }
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

/// The variation from `seed` and `amount`, applied to `knobs` from where
/// they were.
pub fn variation(knobs: &[Knob], seed: u64, amount: f32) -> Variation {
    let values: Values = knobs.iter().map(|knob| (knob.key.clone(), knob.mutated(seed, amount))).collect();
    let travels = knobs.iter().zip(&values).map(|(knob, (_, v))| travel(&knob.from, *v)).collect();
    Variation { seed, amount, values, travels }
}

/// The knobs as they stand, as a variation of nothing.
pub fn as_they_are(knobs: &[Knob]) -> Variation {
    let values: Values = knobs.iter().map(|knob| (knob.key.clone(), knob.from.actual_value())).collect();
    let travels = knobs.iter().zip(&values).map(|(knob, (_, v))| travel(&knob.from, *v)).collect();
    Variation { seed: 0, amount: 0.0, values, travels }
}

/// The input a knob is, if it's still in the patch.
fn input_of(graph: &SynthGraph, user_state: &SynthGraphState, key: &KnobKey) -> Option<InputId> {
    let (&node_id, _) = user_state.node_id_map.iter().find(|(_, &m)| m == key.module)?;
    graph.nodes.get(node_id)?.get_input(&key.param).ok()
}

/// Sets each knob of a variation that's still in the patch. Returns how
/// many it found.
pub fn apply(graph: &mut SynthGraph, user_state: &SynthGraphState, values: &Values) -> usize {
    let mut found = 0;
    for (key, value) in values {
        let Some(input) = input_of(graph, user_state, key) else { continue };
        graph.inputs[input].value.set_actual_value(*value);
        found += 1;
    }
    found
}

/// Whether the knobs stand where a variation left them, as they do after
/// it's made or restored, until something (undo, a hand) moves one.
pub fn matches(graph: &SynthGraph, user_state: &SynthGraphState, values: &Values) -> bool {
    values.iter().all(|(key, value)| {
        input_of(graph, user_state, key)
            .is_some_and(|input| (graph.get_input(input).value.actual_value() - value).abs() <= 1e-4 * value.abs().max(1.0))
    })
}

/// A seed no earlier press used, from the clock and a count.
pub fn fresh_seed(count: u64) -> u64 {
    let nanos = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut rng = Pcg32::new(nanos ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    ((rng.bipolar().to_bits() as u64) << 32) | rng.bipolar().to_bits() as u64
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

        fn knobs(&self, selection: &[NodeId], switches: bool) -> Vec<Knob> {
            knobs(&self.editor.graph, &self.user_state, selection, switches)
        }

        fn names(&self, selection: &[NodeId], switches: bool) -> Vec<String> {
            self.knobs(selection, switches).into_iter().map(|k| k.key.param).collect()
        }

        fn param(&self, node_id: NodeId, name: &str) -> f32 {
            self.editor.graph.get_input(self.editor.graph[node_id].get_input(name).unwrap()).value.actual_value()
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_variation() {
        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let knobs = rig.knobs(&[filter], false);
        let a = variation(&knobs, 42, 0.3);
        let b = variation(&knobs, 42, 0.3);
        let c = variation(&knobs, 43, 0.3);
        assert_eq!(a.values, b.values);
        assert_ne!(a.values, c.values);
        // ...and it moved something
        assert_ne!(a.values, as_they_are(&knobs).values);
    }

    #[test]
    fn a_knob_moves_no_further_than_the_amount_round_its_travel() {
        let mut rig = Rig::new();
        let osc = rig.add("osc.sine");
        for knob in rig.knobs(&[osc], false) {
            let before = travel(&knob.from, knob.from.actual_value());
            for seed in 0..50 {
                let after = travel(&knob.from, knob.mutated(seed, NUDGE));
                assert!((after - before).abs() <= NUDGE + 1e-4, "{} went from {before} to {after}", knob.key.param);
            }
        }
    }

    #[test]
    fn a_hold_grows_from_a_nudge_to_a_jump() {
        assert_eq!(amount(0.0), NUDGE);
        assert!(amount(1.0) > NUDGE && amount(1.0) < JUMP);
        assert_eq!(amount(RAMP_SECONDS), JUMP);
        assert_eq!(amount(60.0), JUMP);
    }

    #[test]
    fn steps_past_an_end_fold_back() {
        assert!((fold(1.2) - 0.8).abs() < 1e-6);
        assert!((fold(-0.25) - 0.25).abs() < 1e-6);
        assert_eq!(fold(0.5), 0.5);
    }

    #[test]
    fn patched_knobs_loud_knobs_and_the_output_stay_put() {
        let mut rig = Rig::new();
        let lfo = rig.add("mod.lfo");
        let filter = rig.add("filter.svf");
        let out = rig.add("output.audio");
        let mixer = rig.add("util.mixer");

        let free = rig.names(&[filter], false);
        assert!(free.contains(&"Cutoff".to_string()), "{free:?}");
        assert!(!free.contains(&"Drive".to_string()));

        // A cable into Cutoff takes it out
        let graph = &mut rig.editor.graph;
        let output = graph[lfo].outputs.first().unwrap().1;
        let cutoff = graph[filter].get_input("Cutoff").unwrap();
        graph.add_connection(output, cutoff, 0);
        assert!(!rig.names(&[filter], false).contains(&"Cutoff".to_string()));

        assert!(rig.names(&[out], true).is_empty());
        let mixer = rig.names(&[mixer], true);
        assert!(mixer.iter().all(|name| !name.contains("Level") && name != "Master"), "{mixer:?}");
        assert!(mixer.iter().all(|name| !name.starts_with("Mute") && !name.starts_with("Solo")), "{mixer:?}");
    }

    #[test]
    fn switches_move_only_when_asked() {
        let mut rig = Rig::new();
        let osc = rig.add("osc.sine");
        let knobs = rig.names(&[osc], false);
        assert!(!knobs.contains(&"Waveform".to_string()) && !knobs.contains(&"Octave".to_string()));
        let all = rig.names(&[osc], true);
        assert!(all.contains(&"Waveform".to_string()) && all.contains(&"Octave".to_string()), "{all:?}");

        // A whole-number knob lands on a whole number, and a dropdown on a choice
        for knob in rig.knobs(&[osc], true) {
            for seed in 0..20 {
                let v = knob.mutated(seed, JUMP);
                if knob.key.param == "Octave" || knob.key.param == "Waveform" {
                    assert_eq!(v, v.round());
                    let (min, max) = knob.from.range();
                    assert!((min..=max).contains(&v));
                }
            }
        }
    }

    #[test]
    fn a_group_mutates_the_modules_inside_it() {
        let mut rig = Rig::new();
        let osc = rig.add("osc.sine");
        let filter = rig.add("filter.svf");
        let made = groups::group(&mut rig.editor, &[osc, filter], GroupId(1), "Voice", Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 300.0)))
            .expect("grouped");
        let group = made.node;
        let inside = rig.knobs(&[group], false);
        let modules: std::collections::BTreeSet<_> = inside.iter().map(|k| k.key.module).collect();
        assert_eq!(modules.len(), 2);

        // Applying a variation sets the knobs inside
        let cutoff = rig.param(filter, "Cutoff");
        let moved = variation(&inside, 7, JUMP);
        assert_eq!(apply(&mut rig.editor.graph, &rig.user_state, &moved.values), inside.len());
        assert_ne!(rig.param(filter, "Cutoff"), cutoff);
        assert!(matches(&rig.editor.graph, &rig.user_state, &moved.values));
        // ...and the original puts them back
        let original = as_they_are(&inside);
        assert!(!matches(&rig.editor.graph, &rig.user_state, &original.values));
        apply(&mut rig.editor.graph, &rig.user_state, &original.values);
        assert_eq!(rig.param(filter, "Cutoff"), cutoff);
        assert!(matches(&rig.editor.graph, &rig.user_state, &original.values));
    }

    #[test]
    fn a_held_mutation_is_one_undo_step() {
        use crate::app::undo::History;
        use web_time::{Duration, Instant};

        let mut rig = Rig::new();
        let filter = rig.add("filter.svf");
        let mut history = History::default();
        history.reset(&rig.editor, &rig.user_state);
        let cutoff = rig.param(filter, "Cutoff");

        // Held for a while, the knobs moving every frame, as the app does
        let knobs = rig.knobs(&[filter], false);
        let mut now = Instant::now();
        for frame in 0..30 {
            let moved = variation(&knobs, 9, amount(frame as f32 / 10.0));
            apply(&mut rig.editor.graph, &rig.user_state, &moved.values);
            now += Duration::from_millis(100);
            history.record(&rig.editor, &rig.user_state, true, now);
        }
        history.name_next("Mutate");
        history.record(&rig.editor, &rig.user_state, false, now);
        assert_ne!(rig.param(filter, "Cutoff"), cutoff);

        assert_eq!(history.undo_label(), Some("Mutate"));
        history.undo(&mut rig.editor, &mut rig.user_state).expect("a step");
        assert_eq!(rig.param(filter, "Cutoff"), cutoff);
        assert_eq!(history.undo_label(), None);
    }

    /// An Instrument playing `profile`, each knob holding one of its first
    /// twelve parameters, as choosing it on the face does.
    fn instrument(rig: &mut Rig, profile: &str) -> (NodeId, Vec<hardware::Parameter>) {
        let node = rig.add(crate::modules::instrument::ID);
        let profile = hardware::known().find(profile).expect("ships").profile.clone();
        for (key, text) in instrument::choose(&profile) {
            if !text.is_empty() {
                rig.editor.graph[node].user_data.labels.insert(key, text);
            }
        }
        (node, profile.parameters)
    }

    /// The parameters a mutation of `node` moves, by the names they go by.
    fn held_names(rig: &Rig, node: NodeId, parameters: &[hardware::Parameter], switches: bool) -> Vec<String> {
        rig.names(&[node], switches)
            .iter()
            .map(|slot| match instrument::slot_named(slot) {
                Some(slot) => parameters[slot].name.clone(),
                None => slot.clone(),
            })
            .collect()
    }

    #[test]
    fn an_instrument_mutates_only_what_its_profile_lets_it() {
        // The volca fm: Transpose and Velocity are marked to stay, the arp's
        // type and division are named steps, and the twelfth knob is empty
        let mut rig = Rig::new();
        let (fm, parameters) = instrument(&mut rig, "volca fm");
        assert!(parameters.len() < crate::modules::instrument::SLOTS);
        let knobs = held_names(&rig, fm, &parameters, false);
        assert!(knobs.contains(&"Mod Attack".to_string()) && knobs.contains(&"LFO Rate".to_string()), "{knobs:?}");
        for kept in ["Transpose", "Velocity", "Arp Type", "Arp Div"] {
            assert!(!knobs.contains(&kept.to_string()), "{kept} moved: {knobs:?}");
        }
        let all = held_names(&rig, fm, &parameters, true);
        assert!(all.contains(&"Arp Type".to_string()) && all.contains(&"Arp Div".to_string()), "{all:?}");
        assert!(!all.contains(&"Transpose".to_string()) && !all.contains(&"Velocity".to_string()), "{all:?}");
        // Its Port and Channel say where it plays, not how it sounds
        assert!(!all.contains(&"Port".to_string()) && !all.contains(&"Channel".to_string()), "{all:?}");
        // ...and no empty knob, or one past the profile's
        assert_eq!(all.len(), parameters.iter().filter(|p| p.mutate).count(), "{all:?}");
    }

    #[test]
    fn a_stepped_instrument_knob_moves_by_whole_steps() {
        let mut rig = Rig::new();
        let (fm, parameters) = instrument(&mut rig, "volca fm");
        let slot = parameters.iter().position(|p| p.name == "Arp Type").unwrap();
        let count = instrument::steps_of(&parameters[slot]).unwrap() as f32;
        let knob = rig.knobs(&[fm], true).into_iter().find(|k| instrument::slot_named(&k.key.param) == Some(slot)).unwrap();
        let from = knob.from.actual_value();
        let mut moved = 0;
        for seed in 0..40 {
            let to = knob.mutated(seed, JUMP);
            if to != from {
                moved += 1;
                // The middle of a step, so the device lands on a value of its own
                let step = to * count - 0.5;
                assert!((step - step.round()).abs() < 1e-4, "{to} isn't a step's middle");
            }
        }
        assert!(moved > 0, "a jump moves it");
        // A nudge moves it a step at the most, if at all
        let at = (from * count).floor();
        for seed in 0..40 {
            assert!(((knob.mutated(seed, NUDGE) * count).floor() - at).abs() <= 1.0);
        }
    }

    #[test]
    fn a_knob_that_nudges_the_device_stays_put() {
        // The FANTOM's Cutoff moves its sound by an offset: undo couldn't
        // walk it back
        let mut rig = Rig::new();
        let (fantom, parameters) = instrument(&mut rig, "FANTOM-06");
        let moved = held_names(&rig, fantom, &parameters, true);
        assert!(!moved.is_empty());
        for p in parameters.iter().take(12).filter(|p| p.relative || !p.mutate) {
            assert!(!moved.contains(&p.name), "{} moved: {moved:?}", p.name);
        }
    }

    #[test]
    fn a_kit_mutates_its_knobs_but_not_which_track_they_turn() {
        let mut rig = Rig::new();
        let kit = rig.add(crate::modules::kit::ID);
        let profile = hardware::known().find("Digitakt").expect("ships").profile.clone();
        for (key, text) in instrument::choose_kit(&profile) {
            if !text.is_empty() {
                rig.editor.graph[kit].user_data.labels.insert(key, text);
            }
        }
        let all = rig.names(&[kit], true);
        assert!(all.iter().any(|name| instrument::slot_named(name).is_some()), "{all:?}");
        assert!(all.iter().all(|name| instrument::slot_named(name).is_some()), "only its knobs: {all:?}");
    }

    #[test]
    fn the_strip_keeps_the_last_eight() {
        let mut strip = Strip::new(&[], as_they_are(&[]));
        for seed in 0..12 {
            strip.keep(variation(&[], seed, NUDGE));
        }
        assert_eq!(strip.variations.len(), KEPT);
        assert_eq!(strip.variations.front().unwrap().seed, 4);
    }
}
