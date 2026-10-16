//! Node templates for the synthesizer graph.
//!
//! Templates are generated from the module registry. Each `DspModule`
//! declares its ports and parameters, and [`super::module_ui`] adds a few
//! presentation hints on top. Adding a module needs only the module file and
//! its registration in `create_module_registry`.
//!
//! # How a module becomes a node
//!
//! - Each input port becomes a jack (`ConnectionOnly`).
//! - Each parameter becomes a value (`ConstantOnly`): a knob for continuous
//!   values, an inline checkbox or dropdown for toggles and choices.
//! - A port named after a parameter ("Cutoff", or "Time CV" for "Time") merges
//!   with it into one input that has both a jack and a knob
//!   (`ConnectionOrConstant`).
//!
//! Inputs keep both the port order and the parameter order of the module,
//! which is what [`super::port_mapping`] relies on to translate editor inputs
//! into engine port and parameter indices.

use std::borrow::Cow;
use egui_node_graph2::{Graph, InputParamKind, NodeId, NodeTemplateIter, NodeTemplateTrait};

use crate::dsp::bypass::can_bypass;
use crate::dsp::{ModuleCategory, ParameterDefinition, ParameterDisplay, SignalType};
use super::catalog::{self, ModuleSpec};
use super::module_ui::{self, KnobHint, ModuleUi};
use super::node_data::KnobInputMode;
use super::{KnobParam, SynthDataType, SynthGraph, SynthGraphState, SynthNodeData, SynthValueType};

/// A module type that can be added to the graph.
#[derive(Clone, Copy, Debug)]
pub struct SynthNodeTemplate {
    spec: &'static ModuleSpec,
}

impl PartialEq for SynthNodeTemplate {
    fn eq(&self, other: &Self) -> bool {
        self.module_id() == other.module_id()
    }
}

impl Eq for SynthNodeTemplate {}

impl SynthNodeTemplate {
    /// Finds the template for a registered module ID, e.g. for patch loading.
    pub fn from_module_id(module_id: &str) -> Option<Self> {
        catalog::module(module_id).map(|spec| Self { spec })
    }

    /// The module's ID, matching its `DspModule::info()`.
    pub fn module_id(&self) -> &'static str {
        self.spec.info.id
    }

    /// The module's display name, e.g. "Oscillator".
    pub fn name(&self) -> &'static str {
        self.spec.info.name
    }

    /// The module's category, for menus and header colour.
    pub fn category(&self) -> ModuleCategory {
        self.spec.info.category
    }

    /// One-line description of what the module does, for tooltips.
    pub fn description(&self) -> &'static str {
        self.spec.info.description
    }

    /// Names of this module's parameters, in parameter-index order.
    pub fn parameter_names(&self) -> Vec<String> {
        self.spec.parameters.iter().map(|p| p.name.to_string()).collect()
    }

    /// Default values of this module's parameters, in parameter-index order.
    pub fn parameter_defaults(&self) -> Vec<f32> {
        self.spec.parameters.iter().map(|p| p.default).collect()
    }

    /// Number of leading parameters driven by live input (computer keyboard,
    /// MIDI) rather than by the graph.
    pub fn live_parameter_count(&self) -> usize {
        self.ui().live_params
    }

    /// Presentation hints, or the defaults if the module has no entry.
    fn ui(&self) -> &'static ModuleUi {
        module_ui::module_ui(self.module_id()).unwrap_or_else(module_ui::default_ui)
    }

    /// The knob row. Without hints, every visible continuous parameter gets a knob.
    fn knob_hints(&self) -> Vec<KnobHint> {
        if let Some(ui) = module_ui::module_ui(self.module_id()) {
            return ui.knobs.to_vec();
        }
        let ui = self.ui();
        self.spec
            .parameters
            .iter()
            .enumerate()
            .filter(|(i, p)| *i >= ui.live_params && is_continuous(p) && !ui.is_hidden(p.name))
            .map(|(_, p)| KnobHint { param: p.name, label: p.name, modulatable: false })
            .collect()
    }

    fn knob_params(&self) -> Vec<KnobParam> {
        self.knob_hints()
            .into_iter()
            .map(|hint| {
                let has_cv = self
                    .spec
                    .parameters
                    .iter()
                    .position(|p| p.name == hint.param)
                    .is_some_and(|index| self.spec.has_cv_input(index));
                let mode = match (has_cv, hint.modulatable) {
                    (false, _) => KnobInputMode::KnobOnly,
                    (true, false) => KnobInputMode::Exposed,
                    (true, true) => KnobInputMode::Modulatable,
                };
                KnobParam::with_mode(hint.param, hint.label, mode)
            })
            .collect()
    }

    /// Adds a parameter-only input (no jack).
    fn add_parameter(&self, graph: &mut SynthGraph, node_id: NodeId, def: &ParameterDefinition) {
        let ui = self.ui();
        let hidden = ui.is_hidden(def.name);
        let label = if hidden { "" } else { ui.label_for(def.name).unwrap_or(def.name) };
        let value = SynthValueType::from_definition(def, label);
        // Knobs live in the bottom row; only toggles and dropdowns render inline
        let inline = !hidden
            && matches!(value, SynthValueType::Toggle { .. } | SynthValueType::Select { .. });
        graph.add_input_param(
            node_id,
            def.name.to_string(),
            SynthDataType::new(SignalType::Control),
            value,
            InputParamKind::ConstantOnly,
            inline,
        );
    }
}

/// Whether a parameter is a continuous value (a knob), not a toggle or choice.
fn is_continuous(def: &ParameterDefinition) -> bool {
    matches!(
        def.display,
        ParameterDisplay::Linear { .. } | ParameterDisplay::Logarithmic { .. } | ParameterDisplay::Stepped { .. }
    )
}

/// Iterator over all available node templates.
pub struct AllNodeTemplates;

impl NodeTemplateIter for AllNodeTemplates {
    type Item = SynthNodeTemplate;

    fn all_kinds(&self) -> Vec<Self::Item> {
        catalog::modules().iter().map(|spec| SynthNodeTemplate { spec }).collect()
    }
}

impl AllNodeTemplates {
    /// Returns all templates grouped by category.
    ///
    /// Categories are returned in a logical display order:
    /// Sources, Filters, Modulation, Effects, Utilities, Output.
    /// Only includes categories that have at least one template.
    pub fn by_category() -> Vec<(ModuleCategory, Vec<SynthNodeTemplate>)> {
        use std::collections::HashMap;

        // Collect templates by category
        let mut map: HashMap<ModuleCategory, Vec<SynthNodeTemplate>> = HashMap::new();
        for template in Self.all_kinds() {
            map.entry(template.category())
                .or_default()
                .push(template);
        }

        // Define display order for categories
        let category_order = [
            ModuleCategory::Source,
            ModuleCategory::Filter,
            ModuleCategory::Modulation,
            ModuleCategory::Effect,
            ModuleCategory::Utility,
            ModuleCategory::Output,
        ];

        // Build result in display order, excluding empty categories
        category_order
            .into_iter()
            .filter_map(|cat| map.remove(&cat).map(|templates| (cat, templates)))
            .collect()
    }
}

impl NodeTemplateTrait for SynthNodeTemplate {
    type NodeData = SynthNodeData;
    type DataType = SynthDataType;
    type ValueType = SynthValueType;
    type UserState = SynthGraphState;
    type CategoryType = ModuleCategory;

    fn node_finder_label(&self, _user_state: &mut Self::UserState) -> Cow<'_, str> {
        Cow::Borrowed(self.spec.info.name)
    }

    fn node_finder_categories(&self, _user_state: &mut Self::UserState) -> Vec<Self::CategoryType> {
        vec![self.category()]
    }

    fn node_graph_label(&self, _user_state: &mut Self::UserState) -> String {
        self.spec.info.name.to_string()
    }

    fn user_data(&self, _user_state: &mut Self::UserState) -> Self::NodeData {
        let ui = self.ui();
        let monitored_outputs = ui
            .monitor
            .iter()
            .filter_map(|name| self.spec.outputs().position(|port| port.name == *name))
            .collect();
        SynthNodeData::new(self.module_id(), self.spec.info.name, self.category())
            .with_description(self.description())
            .with_knob_params(self.knob_params())
            .with_knobs_per_row(ui.knobs_per_row)
            .with_monitored_outputs(monitored_outputs)
            .with_display(ui.display)
            .with_bypassable(can_bypass(self.category(), &self.spec.ports))
    }

    fn build_node(
        &self,
        graph: &mut Graph<Self::NodeData, Self::DataType, Self::ValueType>,
        _user_state: &mut Self::UserState,
        node_id: NodeId,
    ) {
        let spec = self.spec;
        let params = &spec.parameters;
        let mut next_param = 0;

        for port in spec.inputs() {
            match spec.paired_parameter(port) {
                Some(index) => {
                    // Parameters before this one come first, so both port order
                    // and parameter order survive the interleaving
                    assert!(
                        index >= next_param,
                        "{}: port '{}' pairs with parameter '{}' out of parameter order",
                        spec.info.id,
                        port.name,
                        params[index].name
                    );
                    for def in &params[next_param..index] {
                        self.add_parameter(graph, node_id, def);
                    }
                    let def = &params[index];
                    graph.add_input_param(
                        node_id,
                        def.name.to_string(),
                        SynthDataType::new(port.signal_type),
                        SynthValueType::from_definition(def, ""),
                        InputParamKind::ConnectionOrConstant,
                        true, // Jack shown; its knob lives in the bottom row
                    );
                    next_param = index + 1;
                }
                None => {
                    graph.add_input_param(
                        node_id,
                        port.name.to_string(),
                        SynthDataType::new(port.signal_type),
                        SynthValueType::Port,
                        InputParamKind::ConnectionOnly,
                        true,
                    );
                }
            }
        }

        for def in &params[next_param..] {
            self.add_parameter(graph, node_id, def);
        }

        for port in spec.outputs() {
            graph.add_output_param(node_id, port.name.to_string(), SynthDataType::new(port.signal_type));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::port_mapping;

    fn template(module_id: &str) -> SynthNodeTemplate {
        SynthNodeTemplate::from_module_id(module_id).unwrap()
    }

    fn build(template: SynthNodeTemplate) -> (SynthGraph, NodeId) {
        let mut graph = SynthGraph::default();
        let mut user_state = SynthGraphState::new();
        let node_id = graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        (graph, node_id)
    }

    /// Renders everything a template builds (ports, values, knobs, monitors)
    /// as stable text, one line per item.
    fn snapshot(template: SynthNodeTemplate) -> String {
        use std::fmt::Write;
        let (graph, node_id) = build(template);
        let mut user_state = SynthGraphState::new();
        let node = &graph.nodes[node_id];
        let data = &node.user_data;
        let mut out = String::new();
        writeln!(
            out,
            "== {} \"{}\" finder=\"{}\" {:?}",
            template.module_id(),
            node.label,
            template.node_finder_label(&mut user_state),
            template.category()
        )
        .unwrap();
        for (name, id) in &node.inputs {
            let input = graph.get_input(*id);
            let value = match &input.value {
                SynthValueType::Port => "port".to_string(),
                SynthValueType::Toggle { value, label } => format!("toggle {value} \"{label}\""),
                SynthValueType::Select { value, options, label } => {
                    format!("select {value} {options:?} \"{label}\"")
                }
                SynthValueType::Number { value, spec } => format!(
                    "num {value} [{}..{}] {}{}{}",
                    spec.min,
                    spec.max,
                    spec.unit,
                    if spec.logarithmic { " log" } else { "" },
                    if spec.stepped { " stepped" } else { "" }
                ),
            };
            writeln!(
                out,
                "in   {:<18} {:<8} {:<20} inline={:<5} {}",
                name,
                input.typ.signal_type().name(),
                format!("{:?}", input.kind),
                input.shown_inline,
                value.trim_end()
            )
            .unwrap();
        }
        for (name, id) in &node.outputs {
            writeln!(out, "out  {:<18} {}", name, graph.get_output(*id).typ.signal_type().name()).unwrap();
        }
        for kp in &data.knob_params {
            writeln!(out, "knob {:<18} \"{}\" {:?}", kp.param_name, kp.label, kp.input_mode).unwrap();
        }
        if !data.monitored_outputs.is_empty() {
            writeln!(out, "monitor {:?}", data.monitored_outputs).unwrap();
        }
        if data.display != super::super::node_data::NodeDisplay::None {
            writeln!(out, "display {:?}", data.display).unwrap();
        }
        out
    }

    #[test]
    fn test_every_jack_lies_on_its_own_row() {
        // The node editor keeps a row height for each input shown beside the
        // jacks, but lays the jacks along every input in turn: past an input
        // with nothing to show there (a knob-only parameter), each jack sits
        // a row too high and the last is never drawn. So jacks come first
        for template in AllNodeTemplates.all_kinds() {
            let (graph, node_id) = build(template);
            let inputs: Vec<_> = graph.nodes[node_id].inputs.iter().map(|(name, id)| (name, graph.get_input(*id))).collect();
            let Some(first_hidden) = inputs.iter().position(|(_, input)| !input.shown_inline) else { continue };
            for (name, input) in &inputs[first_hidden..] {
                assert!(
                    matches!(input.kind, InputParamKind::ConstantOnly),
                    "{}: the {name} jack comes after {}, which has no row beside the jacks",
                    template.module_id(),
                    inputs[first_hidden].0
                );
            }
        }
    }

    #[test]
    fn test_templates_match_snapshot() {
        // Every module's editor node, rendered as text. Regenerate with
        // UPDATE_SNAPSHOTS=1 and review the diff before committing it
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/graph/fixtures/node_templates.snap");
        let actual: String = AllNodeTemplates
            .all_kinds()
            .into_iter()
            .map(snapshot)
            .collect::<Vec<_>>()
            .join("\n");
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(path).expect("snapshot missing; run with UPDATE_SNAPSHOTS=1");
        let expected = expected.replace("\r\n", "\n");
        if actual != expected {
            let diff: Vec<String> = expected
                .lines()
                .zip(actual.lines())
                .filter(|(e, a)| e != a)
                .map(|(e, a)| format!("- {e}\n+ {a}"))
                .collect();
            panic!(
                "node templates changed ({} vs {} lines):\n{}",
                expected.lines().count(),
                actual.lines().count(),
                diff.join("\n")
            );
        }
    }

    #[test]
    fn test_parameter_names_are_unique_per_module() {
        // Patches save parameters by name, so a duplicate would make one
        // parameter's saved value land on the other
        for template in AllNodeTemplates.all_kinds() {
            let names = template.parameter_names();
            let unique: std::collections::HashSet<_> = names.iter().collect();
            assert_eq!(unique.len(), names.len(), "{}: {:?}", template.module_id(), names);
        }
    }

    #[test]
    fn test_editor_indices_match_dsp_definition() {
        // The editor's jacks, outputs and values must resolve to the module's
        // own port and parameter indices, in order
        for template in AllNodeTemplates.all_kinds() {
            let id = template.module_id();
            let spec = template.spec;
            let (graph, node_id) = build(template);
            let node = &graph.nodes[node_id];

            let params: Vec<&str> = port_mapping::parameter_inputs(&graph, node_id)
                .into_iter()
                .map(|input_id| {
                    let (name, _) = node.inputs.iter().find(|(_, i)| *i == input_id).unwrap();
                    name.as_str()
                })
                .collect();
            let expected: Vec<&str> = spec.parameters.iter().map(|p| p.name).collect();
            assert_eq!(params, expected, "{id}: parameter order");

            for (_, input_id) in &node.inputs {
                if let Some(index) = port_mapping::input_port_index(&graph, node_id, *input_id) {
                    let input = graph.get_input(*input_id);
                    let port = &spec.ports[index];
                    assert!(port.is_input(), "{id}: jack {index} is not a DSP input");
                    assert_eq!(input.typ.signal_type(), port.signal_type, "{id}: jack {index} type");
                }
            }
            let jacks = node
                .inputs
                .iter()
                .filter(|(_, i)| port_mapping::input_port_index(&graph, node_id, *i).is_some())
                .count();
            assert_eq!(jacks, spec.inputs().count(), "{id}: jack count");

            for (name, output_id) in &node.outputs {
                let index = port_mapping::output_port_index(&graph, node_id, *output_id).unwrap();
                let port = &spec.ports[index];
                assert!(port.is_output() && port.name == name, "{id}: output '{name}' maps to '{}'", port.name);
            }
        }
    }

    #[test]
    fn test_values_start_at_dsp_defaults() {
        for template in AllNodeTemplates.all_kinds() {
            let (graph, node_id) = build(template);
            let inputs = port_mapping::parameter_inputs(&graph, node_id);
            for (input_id, def) in inputs.into_iter().zip(&template.spec.parameters) {
                let value = graph.get_input(input_id).value.actual_value();
                assert_eq!(value, def.default, "{}: {}", template.module_id(), def.name);
            }
        }
    }

    #[test]
    fn test_hints_name_real_modules_and_parameters() {
        // A typo in the hint table would silently drop a knob or label
        for template in AllNodeTemplates.all_kinds() {
            let Some(ui) = module_ui::module_ui(template.module_id()) else { continue };
            let id = template.module_id();
            let spec = template.spec;
            let param = |name: &str| spec.parameters.iter().position(|p| p.name == name);

            for knob in ui.knobs {
                let index = param(knob.param).unwrap_or_else(|| panic!("{id}: no parameter '{}'", knob.param));
                assert!(is_continuous(&spec.parameters[index]), "{id}: '{}' is not continuous", knob.param);
                if knob.modulatable {
                    assert!(spec.has_cv_input(index), "{id}: '{}' has no CV input to modulate", knob.param);
                }
            }
            for (name, _) in ui.labels {
                assert!(param(name).is_some(), "{id}: label for unknown parameter '{name}'");
            }
            for name in ui.monitor {
                assert!(spec.outputs().any(|p| p.name == *name), "{id}: no output '{name}'");
            }

            // Every continuous parameter needs a control, unless it's deliberately hidden
            for (index, def) in spec.parameters.iter().enumerate() {
                let controlled = !is_continuous(def)
                    || ui.knobs.iter().any(|k| k.param == def.name)
                    || ui.is_hidden(def.name)
                    || index < ui.live_params;
                assert!(controlled, "{id}: '{}' has no knob and isn't hidden", def.name);
            }
        }
        for ui in super::super::module_ui::all() {
            assert!(catalog::module(ui.module_id).is_some(), "hints for unregistered '{}'", ui.module_id);
        }
    }

    #[test]
    fn test_module_without_hints_gets_a_knob_per_parameter() {
        // Sample & Hold has no hint entry: its Slew knob comes from the DSP alone
        let template = template("util.sample_hold");
        assert!(module_ui::module_ui(template.module_id()).is_none());
        let (graph, node_id) = build(template);
        let knobs = &graph.nodes[node_id].user_data.knob_params;
        assert_eq!(knobs.len(), 1);
        assert_eq!(knobs[0].param_name, "Slew");
        assert_eq!(knobs[0].input_mode, KnobInputMode::KnobOnly);
    }

    #[test]
    fn test_all_templates() {
        let ids: Vec<&str> = AllNodeTemplates.all_kinds().iter().map(|t| t.module_id()).collect();
        assert_eq!(ids.len(), 43);
        for id in ["util.scenes", "fx.slicer", "util.dancer", "source.chopper", "fx.vocoder", "seq.chord", "seq.arranger", "osc.sine", "source.noise", "source.drum", "source.sampler", "source.audio_input", "util.quantizer", "util.divider", "util.logic", "util.looper", "output.audio", "mod.lfo", "mod.slope", "util.mix", "util.mixer", "filter.svf", "filter.ladder", "fx.compressor", "seq.step", "seq.trigger", "fx.tape"] {
            assert!(ids.contains(&id), "{id}");
        }
    }

    #[test]
    fn test_from_module_id() {
        assert_eq!(template("osc.sine").module_id(), "osc.sine");
        assert!(SynthNodeTemplate::from_module_id("not.a.module").is_none());
    }

    #[test]
    fn test_category_and_labels_come_from_module_info() {
        let mut state = SynthGraphState::default();
        assert_eq!(template("osc.sine").category(), ModuleCategory::Source);
        assert_eq!(template("filter.svf").category(), ModuleCategory::Filter);
        assert_eq!(template("fx.reverb").category(), ModuleCategory::Effect);
        assert_eq!(template("output.audio").category(), ModuleCategory::Output);
        assert_eq!(template("filter.svf").node_finder_label(&mut state), "SVF Filter");
        assert_eq!(template("fx.eq").node_graph_label(&mut state), "3-Band EQ");
        assert!(!template("fx.chorus").description().is_empty());
    }
}
