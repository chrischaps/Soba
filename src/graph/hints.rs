//! Tooltips for jacks and knobs.
//!
//! Each DSP module describes its own ports and parameters, so a tooltip is
//! looked up by module ID and name in the [`catalog`] rather than stored on
//! every node.

use eframe::egui::{self, RichText};

use crate::app::theme;
use crate::dsp::{ParameterDefinition, ParameterDisplay, PortDefinition, SignalType};
use super::catalog::{self, ModuleSpec};
use super::value_types::NumberSpec;

/// What a tooltip says about one jack or knob.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    name: &'static str,
    description: &'static str,
    /// The signal a jack carries. Knobs have none.
    signal: Option<SignalType>,
    /// A knob's range, e.g. "20 Hz to 20.0 kHz".
    range: Option<String>,
    /// On a group's face where it's been named: that name, and the module
    /// whose control it is.
    face: Option<(String, String)>,
    /// A knob's, which says how to turn it other than by dragging.
    knob: bool,
}

impl Hint {
    fn port(port: &PortDefinition) -> Self {
        Self { name: port.name, description: port.description, signal: Some(port.signal_type), range: None, face: None, knob: false }
    }

    fn parameter(def: &ParameterDefinition) -> Self {
        let continuous = matches!(
            def.display,
            ParameterDisplay::Linear { .. } | ParameterDisplay::Logarithmic { .. } | ParameterDisplay::Stepped { .. }
        );
        let range = continuous.then(|| {
            let format = NumberSpec::from_definition(def).format();
            format!("{} to {}", format.format(def.min), format.format(def.max))
        });
        Self { name: def.name, description: def.description, signal: None, range, face: None, knob: false }
    }

    /// The tooltip for an editor input, by its name. A jack describes its
    /// port, even when it shares a name with its knob ("Cutoff", or the
    /// "Time CV" port of "Time"); an inline toggle or dropdown, its parameter.
    pub fn input(module_id: &str, name: &str) -> Option<Self> {
        let spec = catalog::module(module_id)?;
        spec.inputs()
            .find(|port| port.name == name || spec.paired_parameter(port).is_some_and(|i| spec.parameters[i].name == name))
            .map(Self::port)
            .or_else(|| find_parameter(spec, name).map(Self::parameter))
    }

    /// The tooltip for an output jack, by its name.
    pub fn output(module_id: &str, name: &str) -> Option<Self> {
        catalog::module(module_id)?.outputs().find(|port| port.name == name).map(Self::port)
    }

    /// The tooltip for a knob, by its parameter's name.
    pub fn knob(module_id: &str, param_name: &str) -> Option<Self> {
        find_parameter(catalog::module(module_id)?, param_name).map(|def| Self { knob: true, ..Self::parameter(def) })
    }

    /// The tooltip for a control a group's face calls `name`: it leads with
    /// that, then names `module` and the parameter it drives.
    pub fn named(mut self, name: &str, module: &str) -> Self {
        self.face = Some((name.to_string(), module.to_string()));
        self
    }

    /// Draws the tooltip: the name with the jack's signal type in its cable
    /// colour, what it does, and a knob's range. A control named on a face
    /// reads "Tine: Oscillator FM Depth".
    pub fn show(&self, ui: &mut egui::Ui) {
        ui.set_max_width(260.0);
        ui.horizontal(|ui| {
            if let Some((name, module)) = &self.face {
                ui.label(RichText::new(format!("{name}:")).strong().color(theme::text::PRIMARY));
                ui.label(RichText::new(format!("{module} {}", self.name)).color(theme::text::SECONDARY));
                return;
            }
            ui.label(RichText::new(self.name).strong().color(theme::text::PRIMARY));
            if let Some(signal) = self.signal {
                ui.label(RichText::new(format!("● {}", signal.name())).small().color(signal.color()));
            }
        });
        if !self.description.is_empty() {
            ui.label(RichText::new(self.description).color(theme::text::PRIMARY));
        }
        if let Some(range) = &self.range {
            ui.label(RichText::new(range).small().color(theme::text::SECONDARY));
        }
        if self.knob {
            ui.label(RichText::new(KNOB_TIP).small().color(theme::text::SECONDARY));
        }
    }
}

/// The ways to turn a knob besides dragging it, under a knob's tooltip.
const KNOB_TIP: &str = "Double-click resets. Alt+scroll turns it, as do the arrow keys once it's clicked. Right-click to type a value";

fn find_parameter<'a>(spec: &'a ModuleSpec, name: &str) -> Option<&'a ParameterDefinition> {
    spec.parameters.iter().find(|p| p.name == name)
}

/// Adds `hint` as the response's tooltip, if there is one.
pub fn attach(response: egui::Response, hint: Option<Hint>) -> egui::Response {
    match hint {
        Some(hint) => response.on_hover_ui(|ui| hint.show(ui)),
        None => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_name_finds_the_jack_and_the_knob() {
        let jack = Hint::input("filter.svf", "Cutoff").unwrap();
        assert_eq!(jack.signal, Some(SignalType::Control));
        assert!(jack.range.is_none());

        let knob = Hint::knob("filter.svf", "Cutoff").unwrap();
        assert!(knob.signal.is_none());
        assert!(knob.range.as_deref().is_some_and(|r| r.contains("Hz")), "{:?}", knob.range);
        assert_ne!(jack.description, knob.description);
    }

    #[test]
    fn every_editor_input_and_output_has_a_hint() {
        use egui_node_graph2::NodeTemplateTrait;
        use super::super::{AllNodeTemplates, SynthGraph, SynthGraphState};
        use egui_node_graph2::NodeTemplateIter;

        for template in AllNodeTemplates.all_kinds() {
            let mut graph = SynthGraph::default();
            let mut user_state = SynthGraphState::new();
            let node_id = graph.add_node(
                template.node_graph_label(&mut user_state),
                template.user_data(&mut user_state),
                |graph, node_id| template.build_node(graph, &mut user_state, node_id),
            );
            let node = &graph.nodes[node_id];
            for (name, _) in &node.inputs {
                assert!(Hint::input(template.module_id(), name).is_some(), "{} input {name}", template.module_id());
            }
            for (name, _) in &node.outputs {
                assert!(Hint::output(template.module_id(), name).is_some(), "{} output {name}", template.module_id());
            }
            for knob in &node.user_data.knob_params {
                assert!(Hint::knob(template.module_id(), &knob.param_name).is_some(), "{} knob {}", template.module_id(), knob.param_name);
            }
        }
    }
}
