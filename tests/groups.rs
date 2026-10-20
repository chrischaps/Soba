//! Groups are an editing convenience: the engine only ever hears the modules
//! and the cables between them. These tests group the example patches in
//! awkward ways (every other module, then a group inside that), and check
//! that the grouped patches render sample for sample what the originals do,
//! and that saving, loading and ungrouping gives back the original patch.

use std::collections::{BTreeMap, BTreeSet};

use egui::{pos2, vec2, Rect};
use egui_node_graph2::NodeId;
use soba::engine::OfflineRenderer;
use soba::graph::groups::{self, GroupId, NodeKind};
use soba::graph::{create_editor_state, SynthGraphEditorState};
use soba::persistence::sample_files::SampleBase;
use soba::persistence::{capture_patch, patch_from_json, renumber_groups, stage_patch, Patch, EXAMPLES};

/// An editor holding a patch, its modules under their patch IDs.
struct Editor {
    editor: SynthGraphEditorState,
    ids: BTreeMap<NodeId, u64>,
    next_group: u64,
}

impl Editor {
    fn open(patch: &Patch) -> Self {
        let mut staged = stage_patch(patch).unwrap();
        assert!(staged.warnings.is_empty(), "{}: {:?}", patch.name, staged.warnings);
        // A patch's own groups take IDs after its modules', as the app gives
        // them on load, so saving can't confuse a group with a module
        let mut next_loaded = patch.all_nodes().iter().map(|n| n.id).max().unwrap_or(0);
        renumber_groups(&mut staged.graph, || {
            next_loaded += 1;
            GroupId(next_loaded)
        });
        let mut editor = create_editor_state();
        editor.graph = std::mem::take(&mut staged.graph);
        let mut ids = BTreeMap::new();
        for node in &staged.nodes {
            editor.node_positions.insert(node.graph_id, pos2(node.position.0, node.position.1));
            editor.node_order.push(node.graph_id);
            ids.insert(node.graph_id, node.patch_id);
        }
        for part in &staged.parts {
            editor.node_positions.insert(part.graph_id, pos2(part.position.0, part.position.1));
            editor.node_order.push(part.graph_id);
        }
        // Clear of any module ID
        let next_group = 1 + patch.all_nodes().iter().map(|n| n.id).max().unwrap_or(0)
            + 1000 * (1 + patch.groups.len() as u64);
        Self { editor, ids, next_group }
    }

    fn save(&self, name: &str) -> Patch {
        let ids = &self.ids;
        let positions = &self.editor.node_positions;
        capture_patch(
            name,
            &self.editor.graph,
            |n| ids.get(&n).copied(),
            |n| positions.get(n).map_or((0.0, 0.0), |p| (p.x, p.y)),
            &[],
        )
    }

    /// The modules on a level, by patch ID.
    /// The modules on a level, but for Scenes modules: one moved into a
    /// group keeps the group's knobs and lets go of the rest, so it would
    /// sound different by design.
    fn modules_on(&self, level: Option<GroupId>) -> Vec<NodeId> {
        groups::level_nodes(&self.editor.graph, level)
            .into_iter()
            .filter(|n| self.editor.graph[*n].user_data.kind == NodeKind::Module)
            .filter(|n| self.editor.graph[*n].user_data.module_id != soba::modules::scenes::ID)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn group(&mut self, nodes: &[NodeId], name: &str) -> GroupId {
        let id = GroupId(self.next_group);
        self.next_group += 1;
        let bounds = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0));
        groups::group(&mut self.editor, nodes, id, name, bounds).expect("grouped");
        id
    }

    fn ungroup_all(&mut self) {
        while let Some(node) = self
            .editor
            .graph
            .nodes
            .iter()
            .find(|(_, n)| n.user_data.parent.is_none() && n.user_data.kind.group().is_some())
            .map(|(id, _)| id)
        {
            groups::ungroup(&mut self.editor, node).unwrap();
        }
    }
}

/// Groups every other module of the patch, then every other module inside
/// that group into a group of its own.
fn group_awkwardly(patch: &Patch) -> Patch {
    let mut editor = Editor::open(patch);
    let top: Vec<NodeId> = editor.modules_on(None).into_iter().step_by(2).collect();
    let outer = editor.group(&top, "Outer");
    let inner: Vec<NodeId> = editor.modules_on(Some(outer)).into_iter().step_by(2).collect();
    if !inner.is_empty() {
        editor.group(&inner, "Inner");
    }
    let grouped = editor.save(&patch.name);
    // Through the file format, as a saved patch would go
    patch_from_json(&serde_json::to_string_pretty(&grouped).unwrap()).unwrap()
}

fn render(patch: &Patch) -> Vec<f32> {
    let (mut renderer, compiled) = OfflineRenderer::from_patch(patch, 48_000.0, 256).unwrap();
    assert!(compiled.warnings.is_empty(), "{}: {:?}", patch.name, compiled.warnings);
    // Examples' samples ship inside the app
    let missing = renderer.load_samples(&compiled, SampleBase::Example);
    assert!(missing.is_empty(), "{}: {:?}", patch.name, missing);
    let out = renderer.render_audition(patch, 3.0);
    out.left.into_iter().chain(out.right).collect()
}

/// Each Noise module, and each Drum (whose snares, claps and hats hiss),
/// takes the next random stream from a count the whole process shares, so
/// two renders in one process never match. Patches with noise are compared
/// with the `render` binary instead, one process each.
fn has_noise(patch: &Patch) -> bool {
    patch.all_nodes().iter().any(|n| matches!(n.module_id.as_str(), "source.noise" | "source.drum"))
}

#[test]
fn grouped_examples_render_sample_for_sample() {
    let mut compared = 0;
    for example in EXAMPLES {
        let patch = example.patch().unwrap();
        if has_noise(&patch) {
            continue;
        }
        compared += 1;
        let grouped = group_awkwardly(&patch);
        assert!(!grouped.groups.is_empty(), "{}: nothing was grouped", example.name);
        assert_eq!(grouped.version, 6, "{}: a patch with groups is v6", example.name);

        let (plain, through_groups) = (render(&patch), render(&grouped));
        assert!(plain.iter().any(|s| *s != 0.0), "{}: rendered silence", example.name);
        let differing = plain.iter().zip(&through_groups).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
        assert_eq!(differing, 0, "{}: {} samples differ once grouped", example.name, differing);
    }
    assert!(compared >= 5, "only {compared} examples compared");
}

/// Renders a patch to a WAV with the `render` binary, in a process of its
/// own, and returns the file.
fn render_in_own_process(patch: &Patch, path: &std::path::Path) -> Vec<u8> {
    let json = path.with_extension("json");
    std::fs::write(&json, serde_json::to_string_pretty(patch).unwrap()).unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_render"))
        .args([json.as_os_str(), path.as_os_str()])
        .args(["--seconds", "3", "--audition"])
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "render failed for {}", patch.name);
    std::fs::read(path).unwrap()
}

#[test]
fn grouped_noise_patches_render_the_same_with_the_render_tool() {
    let dir = std::env::temp_dir().join(format!("soba-groups-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut compared = 0;
    for (i, example) in EXAMPLES.iter().enumerate() {
        let patch = example.patch().unwrap();
        if !has_noise(&patch) {
            continue;
        }
        compared += 1;
        let plain = render_in_own_process(&patch, &dir.join(format!("{i}-plain.wav")));
        let grouped = render_in_own_process(&group_awkwardly(&patch), &dir.join(format!("{i}-grouped.wav")));
        assert!(plain[44..].iter().any(|b| *b != 0), "{}: rendered silence", example.name);
        assert!(plain == grouped, "{}: the grouped patch's WAV differs", example.name);
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(compared >= 1, "no example has noise to compare");
}

/// A patch reduced to what makes it sound and look as it does: modules by
/// ID with their values and places, and the cables between them.
fn essence(patch: &Patch) -> (BTreeMap<u64, String>, BTreeSet<(u64, String, u64, String)>) {
    let nodes = patch
        .all_nodes()
        .into_iter()
        .map(|n| (n.id, format!("{} {:?} {:?} {}", n.module_id, n.position, n.parameters, n.bypassed)))
        .collect();
    let cables = patch
        .connections
        .iter()
        .map(|c| (c.from_node, c.from_port.clone(), c.to_node, c.to_port.clone()))
        .collect();
    (nodes, cables)
}

#[test]
fn group_save_load_ungroup_gives_back_the_patch() {
    for example in EXAMPLES {
        let patch = example.patch().unwrap();
        let grouped = group_awkwardly(&patch);

        let mut editor = Editor::open(&grouped);
        editor.ungroup_all();
        assert!(editor.editor.graph.nodes.values().all(|n| n.user_data.kind == NodeKind::Module));
        let ungrouped = editor.save(&patch.name);
        assert!(ungrouped.groups.is_empty());
        assert_eq!(ungrouped.version, 5, "{}: without groups, a patch stays v5", example.name);
        // Against the patch as this build saves it (with any parameters
        // added since the example was saved), its own groups opened the same way
        let mut original = Editor::open(&patch);
        original.ungroup_all();
        let original = original.save(&patch.name);
        assert_eq!(essence(&ungrouped), essence(&original), "{}", example.name);
    }
}

