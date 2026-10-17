//! Recording a module's sound into a new Sampler, from a right-click: an
//! Audio Input's module menu offers **Record into a new Sampler**, and any
//! other audio output, on its module's menu or its own label's,
//! **Resample into a new Sampler**. The app places the Sampler beside the
//! module, patched from it and listening, and presses its Rec, so the next
//! sound is the take.
//!
//! With a Clock in the patch, it offers whole bars too (*1 bar*, *2 bars*,
//! *4 bars*): the take starts on the next bar line instead of the next sound,
//! stops itself, and lands looped, ready for a key or a gate.

use egui_node_graph2::NodeId;

use crate::dsp::SignalType;
use crate::modules::{audio_input, clock};
use super::{SynthGraph, SynthResponse};

/// What a new Sampler records from a module: a stereo pair of its audio
/// outputs, or one alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Patched into In L; alone, the Sampler records it on both sides.
    pub left: String,
    /// Patched into In R.
    pub right: Option<String>,
}

impl Source {
    /// What the menu calls it among the module's others: "Out" for Out L
    /// and Out R, "Send", "LowPass"; "L/R" for a bare pair.
    pub fn name(&self) -> &str {
        match &self.right {
            Some(_) if self.left == "L" => "L/R",
            Some(_) => self.left.strip_suffix(" L").unwrap_or(&self.left),
            None => &self.left,
        }
    }
}

/// The bar counts the menu offers when a Clock runs. Longer takes (8 and 16
/// bars) are on the Sampler's own Take.
pub const BAR_TAKES: [u32; 3] = [1, 2, 4];


/// Whether a take in bars has a Clock to count them: one in the patch whose
/// Run is on, or patched (it may be switched on by then).
pub fn has_clock(graph: &SynthGraph) -> bool {
    graph.nodes.iter().filter(|(_, node)| node.user_data.module_id == clock::ID).any(|(_, node)| {
        node.get_input("Run").ok().is_none_or(|run| graph.connection(run).is_some() || graph.get_input(run).value.actual_value() > 0.5)
    })
}

/// A bar count's words: "1 bar", "2 bars".
pub fn bar_name(bars: u32) -> String {
    if bars == 1 { "1 bar".into() } else { format!("{bars} bars") }
}

/// The other side of a stereo output, by its name, and whether `name` is
/// the left: Out L with Out R, L with R.
fn partner(name: &str) -> Option<(String, bool)> {
    match name {
        "L" => Some(("R".into(), true)),
        "R" => Some(("L".into(), false)),
        _ => {
            if let Some(stem) = name.strip_suffix(" L") {
                Some((format!("{stem} R"), true))
            } else {
                name.strip_suffix(" R").map(|stem| (format!("{stem} L"), false))
            }
        }
    }
}

/// What a node's audio outputs can be recorded as, in their order: pairs
/// where both sides are there, the rest one at a time.
pub fn sources(graph: &SynthGraph, node_id: NodeId) -> Vec<Source> {
    let Some(node) = graph.nodes.get(node_id) else { return Vec::new() };
    let audio: Vec<&str> = node
        .outputs
        .iter()
        .filter(|(_, id)| graph.get_output(*id).typ.0 == SignalType::Audio)
        .map(|(name, _)| name.as_str())
        .collect();
    let mut found = Vec::new();
    for &name in &audio {
        match partner(name) {
            Some((other, true)) if audio.contains(&other.as_str()) => {
                found.push(Source { left: name.into(), right: Some(other) });
            }
            // The right of a pair comes with its left
            Some((other, false)) if audio.contains(&other.as_str()) => {}
            _ => found.push(Source { left: name.into(), right: None }),
        }
    }
    found
}

/// What recording an output records: the pair it's a side of, or itself.
pub fn source_of(graph: &SynthGraph, node_id: NodeId, port: &str) -> Option<Source> {
    sources(graph, node_id).into_iter().find(|s| s.left == port || s.right.as_deref() == Some(port))
}

/// The menu item's words for a node: an Audio Input records, the rest resample.
pub fn label(graph: &SynthGraph, node_id: NodeId) -> &'static str {
    match graph.nodes.get(node_id).map(|n| n.user_data.module_id) {
        Some(audio_input::ID) => "Record into a new Sampler",
        _ => "Resample into a new Sampler",
    }
}

const HINT: &str = "A Sampler beside it, patched from it and listening: the take starts on the next sound, and Rec stops it";

const BARS_HINT: &str = "A Sampler beside it, patched from it: the take starts on the next bar line, lasts this long and loops";

/// The take's lengths, as a submenu's items when a Clock runs: Free, then
/// whole bars. Returns the bars chosen (0 for Free). Without a Clock, the
/// item is Free alone.
fn length_items(ui: &mut egui::Ui, label: &str, clocked: bool) -> Option<u32> {
    if !clocked {
        return ui.button(label).on_hover_text(HINT).clicked().then_some(0);
    }
    let mut chosen = None;
    ui.menu_button(label, |ui| {
        if ui.button("Free").on_hover_text(HINT).clicked() {
            chosen = Some(0);
        }
        for bars in BAR_TAKES {
            if ui.button(bar_name(bars)).on_hover_text(BARS_HINT).clicked() {
                chosen = Some(bars);
            }
        }
    });
    chosen
}

/// A module menu's item: one for a module with one thing to record, or a
/// submenu naming each; with a Clock, each offers its lengths. Returns
/// what was chosen.
pub fn module_items(ui: &mut egui::Ui, graph: &SynthGraph, node_id: NodeId) -> Option<SynthResponse> {
    let sources = sources(graph, node_id);
    let label = label(graph, node_id);
    let clocked = has_clock(graph);
    let choose = |source: &Source, bars: u32| SynthResponse::RecordInto { node_id, source: source.clone(), bars };
    let chosen = match sources.as_slice() {
        [] => None,
        [source] => length_items(ui, label, clocked).map(|bars| choose(source, bars)),
        _ => {
            let mut chosen = None;
            ui.menu_button(label, |ui| {
                for source in &sources {
                    if let Some(bars) = length_items(ui, source.name(), clocked) {
                        chosen = Some(choose(source, bars));
                    }
                }
            });
            chosen
        }
    };
    if chosen.is_some() {
        ui.close_menu();
    }
    chosen
}

/// An audio output label's item, recording the pair it's a side of.
pub fn port_item(ui: &mut egui::Ui, graph: &SynthGraph, node_id: NodeId, source: &Source) -> Option<SynthResponse> {
    let label = label(graph, node_id);
    let named = match &source.right {
        Some(right) if source.left != "L" => format!("{label} ({} and {right})", source.left),
        _ => label.to_string(),
    };
    let bars = length_items(ui, &named, has_clock(graph))?;
    ui.close_menu();
    Some(SynthResponse::RecordInto { node_id, source: source.clone(), bars })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_node_graph2::NodeTemplateTrait;
    use crate::graph::{SynthGraphState, SynthNodeTemplate};

    fn add(graph: &mut SynthGraph, module_id: &str) -> NodeId {
        let mut user_state = SynthGraphState::new();
        let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
        graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        )
    }

    fn sources_of(module_id: &str) -> Vec<Source> {
        let mut graph = SynthGraph::default();
        let node_id = add(&mut graph, module_id);
        sources(&graph, node_id)
    }

    fn pair(left: &str, right: &str) -> Source {
        Source { left: left.into(), right: Some(right.into()) }
    }

    fn mono(name: &str) -> Source {
        Source { left: name.into(), right: None }
    }

    #[test]
    fn test_stereo_outputs_pair_up_and_the_rest_stand_alone() {
        assert_eq!(sources_of(audio_input::ID), [pair("L", "R")]);
        assert_eq!(sources_of("util.mixer"), [pair("Out L", "Out R"), pair("Send L", "Send R")]);
        assert_eq!(sources_of("source.noise").len(), 3);
        // Audio Output has nothing to record from
        assert!(sources_of("output.audio").is_empty());
    }

    #[test]
    fn test_names_read_as_the_menu_shows_them() {
        assert_eq!(pair("L", "R").name(), "L/R");
        assert_eq!(pair("Send L", "Send R").name(), "Send");
        assert_eq!(mono("LowPass").name(), "LowPass");
    }

    #[test]
    fn test_either_side_of_a_pair_records_the_pair() {
        let mut graph = SynthGraph::default();
        let mixer = add(&mut graph, "util.mixer");
        assert_eq!(source_of(&graph, mixer, "Send R"), Some(pair("Send L", "Send R")));
        assert_eq!(source_of(&graph, mixer, "Out L"), Some(pair("Out L", "Out R")));
    }

    #[test]
    fn test_bars_are_offered_while_a_clock_will_run() {
        let mut graph = SynthGraph::default();
        add(&mut graph, "util.mixer");
        assert!(!has_clock(&graph), "no Clock, nothing to count bars");
        let clock = add(&mut graph, clock::ID);
        assert!(has_clock(&graph));
        let run = graph[clock].get_input("Run").unwrap();
        graph.inputs[run].value.set_actual_value(0.0);
        assert!(!has_clock(&graph), "a Clock switched off never reaches the bar");
    }

    #[test]
    fn test_bar_counts_read_as_the_take_knob_does() {
        use crate::modules::sampler::TAKE_LENGTHS;
        for bars in BAR_TAKES {
            assert!(TAKE_LENGTHS.contains(&bar_name(bars).as_str()), "{bars}");
        }
    }
}
