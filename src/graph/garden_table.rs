//! The module table the Patch Garden draws patches from, outside Rust.
//!
//! A patch file names each node's `module_id` and position, and each
//! cable's port names, but not what a module looks like or what a cable
//! carries. `packaging/web/garden/modules.json` says, for a page drawing a
//! patch as a small cable diagram in JS:
//!
//! - **modules**: per `module_id`, its name, category and header colour,
//!   its input jacks and outputs with the signal each carries, and a
//!   nominal height;
//! - **groups**: the header colours of a group's node and of its Inputs and
//!   Outputs inside. A group's jacks and their signals are in the patch
//!   itself (`GroupData`), so a group's height comes from the same formula;
//! - **frames**: each tint a frame can name, and the one an unknown name
//!   reads as;
//! - **signals**: the colour of each kind of cable;
//! - **nominal**: the height formula, so the page can size a group too.
//!
//! The file is kept like `node_templates.snap`: the test fails when it's
//! stale, and `UPDATE_SNAPSHOTS=1` writes it again.

use egui::Color32;
use egui_node_graph2::{InputParamKind, NodeTemplateIter, NodeTemplateTrait};
use serde_json::{json, Value};

use crate::app::theme;
use crate::dsp::{ModuleCategory, SignalType};
use super::annotations::Tint;
use super::node_data::NodeDisplay;
use super::{group_face, groups, AllNodeTemplates, NodeKind, SynthGraph, SynthGraphState};

/// The nominal height of a node, in unzoomed points: a header and padding,
/// a row per jack beside the ports, a row per four knobs, and room for a
/// display. Fitted to the modules as the editor draws them (median error
/// about 45 of ~350 points; the Mixer, drawn in strips, is the furthest
/// off), which is close enough for a diagram the size of a card.
const BASE: f32 = 110.0;
const ROW: f32 = 18.0;
const KNOB_ROW: f32 = 32.0;
const KNOBS_PER_ROW: usize = 4;
const DISPLAY: f32 = 130.0;

fn nominal_height(rows: usize, knobs: usize, display: bool) -> f32 {
    BASE + ROW * rows as f32 + KNOB_ROW * knobs.div_ceil(KNOBS_PER_ROW) as f32 + if display { DISPLAY } else { 0.0 }
}

fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

const SIGNALS: [SignalType; 5] = [SignalType::Audio, SignalType::Control, SignalType::Gate, SignalType::Midi, SignalType::Bus];

const CATEGORIES: [ModuleCategory; 6] = [
    ModuleCategory::Source,
    ModuleCategory::Filter,
    ModuleCategory::Modulation,
    ModuleCategory::Effect,
    ModuleCategory::Utility,
    ModuleCategory::Output,
];

/// One module's entry, from the node the editor builds for it.
fn module_entry(template: super::SynthNodeTemplate) -> Value {
    let mut graph = SynthGraph::default();
    let mut user_state = SynthGraphState::new();
    let node_id = graph.add_node(
        template.node_graph_label(&mut user_state),
        template.user_data(&mut user_state),
        |graph, node_id| template.build_node(graph, &mut user_state, node_id),
    );
    let node = &graph.nodes[node_id];
    let data = &node.user_data;

    let port = |name: &String, signal: SignalType| json!({ "name": name, "type": signal.name() });
    let inputs: Vec<Value> = node
        .inputs
        .iter()
        .map(|(name, id)| (name, graph.get_input(*id)))
        .filter(|(_, input)| !matches!(input.kind, InputParamKind::ConstantOnly))
        .map(|(name, input)| port(name, input.typ.signal_type()))
        .collect();
    let outputs: Vec<Value> =
        node.outputs.iter().map(|(name, id)| port(name, graph.get_output(*id).typ.signal_type())).collect();
    // Beside the ports, a row for each input shown there (a jack, a
    // toggle or a choice) and each output, side by side
    let rows = node.inputs.iter().filter(|(_, id)| graph.get_input(*id).shown_inline).count().max(node.outputs.len());
    let height = nominal_height(rows, data.knob_params.len(), data.display != NodeDisplay::None);

    json!({
        "name": node.label,
        "category": template.category().name(),
        "color": hex(template.category().color()),
        "inputs": inputs,
        "outputs": outputs,
        "height": height,
    })
}

/// The whole table, as the file holds it.
fn table() -> String {
    let modules: serde_json::Map<String, Value> =
        AllNodeTemplates.all_kinds().into_iter().map(|t| (t.module_id().to_string(), module_entry(t))).collect();
    let table = json!({
        "about": "Generated from Soba's module registry by src/graph/garden_table.rs; \
                  regenerate with UPDATE_SNAPSHOTS=1 cargo test --lib garden_table",
        "signals": SIGNALS.iter().map(|s| (s.name().to_string(), json!(hex(s.color())))).collect::<serde_json::Map<_, _>>(),
        "categories": CATEGORIES.iter().map(|c| (c.name().to_string(), json!(hex(c.color())))).collect::<serde_json::Map<_, _>>(),
        "groups": {
            "color": hex(theme::module::GROUP),
            // Inputs and Outputs, inside a group, wear its colour more quietly
            "inside_color": hex(group_face::header_color(NodeKind::Module)),
            "inputs_id": groups::INPUTS_ID,
            "outputs_id": groups::OUTPUTS_ID,
        },
        "frames": {
            "default": Tint::default().key(),
            "tints": Tint::ALL.iter().map(|t| (t.key().to_string(), json!(hex(t.color())))).collect::<serde_json::Map<_, _>>(),
        },
        "nominal": {
            "width": groups::NOMINAL_NODE_SIZE.x,
            "base": BASE,
            "row": ROW,
            "knob_row": KNOB_ROW,
            "knobs_per_row": KNOBS_PER_ROW,
            "display": DISPLAY,
            "formula": "base + row * max(inputs beside the ports, outputs) + knob_row * ceil(knobs / knobs_per_row) \
                        + display if it has one; a group: its jacks, its pinned knobs, and its miniature as the display",
        },
        "modules": modules,
    });
    serde_json::to_string_pretty(&table).unwrap() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_json_matches_the_registry() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/web/garden/modules.json");
        let actual = table();
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
            std::fs::write(path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(path).expect("modules.json missing; run with UPDATE_SNAPSHOTS=1");
        assert!(
            expected.replace("\r\n", "\n") == actual,
            "packaging/web/garden/modules.json is stale: regenerate it with \
             UPDATE_SNAPSHOTS=1 cargo test --lib garden_table and review the diff"
        );
    }

    #[test]
    fn every_cable_in_the_examples_finds_its_ports() {
        // The page looks each cable's ends up by port name: every cable
        // between two modules in the examples has to be there
        let table: Value = serde_json::from_str(&table()).unwrap();
        let modules = &table["modules"];
        let has = |module: &str, side: &str, port: &str| {
            modules[module][side].as_array().is_some_and(|ports| ports.iter().any(|p| p["name"] == port))
        };
        for example in crate::persistence::EXAMPLES {
            let patch = example.patch().unwrap();
            let mut levels = vec![(&patch.nodes, &patch.connections, &patch.groups)];
            while let Some((nodes, connections, inner)) = levels.pop() {
                let module_of = |id: u64| nodes.iter().find(|n| n.id == id).map(|n| n.module_id.as_str());
                for cable in connections {
                    if let Some(module) = module_of(cable.from_node) {
                        assert!(has(module, "outputs", &cable.from_port), "{}: {module} has no output {}", example.file_name, cable.from_port);
                    }
                    if let Some(module) = module_of(cable.to_node) {
                        assert!(has(module, "inputs", &cable.to_port), "{}: {module} has no input {}", example.file_name, cable.to_port);
                    }
                }
                levels.extend(inner.iter().map(|g| (&g.nodes, &g.connections, &g.groups)));
            }
        }
    }

    #[test]
    fn nominal_heights_grow_with_what_a_module_holds() {
        assert!(nominal_height(4, 0, false) > nominal_height(2, 0, false));
        assert!(nominal_height(2, 5, false) > nominal_height(2, 4, false));
        assert!(nominal_height(2, 4, true) > nominal_height(2, 4, false));
    }
}
