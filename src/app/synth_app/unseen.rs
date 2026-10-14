//! Modules no one has seen yet, measured before they're shown.
//!
//! A module's size is only known once it has been drawn, and the editor
//! only draws the level on show. Until a group had been opened, its
//! modules were taken at a nominal size: its miniature showed short cards,
//! the zoom into it grew the inside out of cards that didn't match, and the
//! inside was framed again once it had been drawn (#245).
//!
//! So each module that's hidden and has never been measured is laid out
//! once, off screen and unpainted, at life size, before the frame is drawn.
//! The deepest go first, so a group inside a group has its insides true on
//! its face by the time it's measured itself.

use std::collections::HashMap;

use eframe::egui::{self, vec2, Pos2, Rect, UiBuilder, Vec2};
use egui_node_graph2::{GraphNodeWidget, NodeId, PanZoom};

use crate::graph::annotation_ui;
use crate::graph::groups::GroupIndex;
use crate::graph::{SynthGraphEditorState, SynthGraphState};
use super::grouping::face_of;

/// Measures, into `sizes` (unzoomed points), each module that's hidden and
/// has never been measured. Returns whether there were any.
pub(super) fn measure(
    editor: &mut SynthGraphEditorState,
    user_state: &mut SynthGraphState,
    sizes: &mut HashMap<NodeId, Vec2>,
    ui: &mut egui::Ui,
) -> bool {
    let graph = &editor.graph;
    let index = GroupIndex::of(graph);
    let mut unseen: Vec<(usize, NodeId)> = editor
        .node_order
        .iter()
        .copied()
        .filter(|n| user_state.hidden.contains(n) && !sizes.contains_key(n))
        .map(|n| (index.path(graph, graph[n].user_data.parent).map_or(0, |path| path.len()), n))
        .collect();
    if unseen.is_empty() {
        return false;
    }
    // Deepest first, so a group's face shows its insides measured
    unseen.sort_by_key(|&(depth, _)| std::cmp::Reverse(depth));

    // Past the bottom right of the screen, and clipped away, so nothing
    // there can be pointed at
    let ctx = ui.ctx().clone();
    let away = ctx.screen_rect().max + vec2(1000.0, 1000.0);
    let mut offscreen =
        ui.new_child(UiBuilder::new().id_salt("unseen modules").max_rect(Rect::from_min_size(away, Vec2::splat(1.0))).invisible());
    let life_size = PanZoom { zoom: 1.0, ..PanZoom::default() };
    let zoom = std::mem::replace(&mut user_state.zoom, 1.0);
    // A node takes the outline it drew last time as its own, so each is
    // drawn twice, and its outline is registered twice in one frame
    let warn_on_id_clash = ctx.options(|o| o.warn_on_id_clash);
    ctx.options_mut(|o| o.warn_on_id_clash = false);

    let mut faces = Vec::new();
    for (_, node) in unseen {
        if let Some(id) = editor.graph[node].user_data.kind.group() {
            let positions = &editor.node_positions;
            let face = face_of(&editor.graph, |n| positions.get(n).copied(), sizes, editor.pan_zoom.zoom, &index, id);
            user_state.group_faces.insert(id, face);
            faces.push(id);
        }
        for _ in 0..2 {
            // Placed by `pan` alone
            let mut at = Pos2::ZERO;
            let _ = GraphNodeWidget {
                position: &mut at,
                graph: &mut editor.graph,
                port_locations: &mut Default::default(),
                conn_locations: &mut Default::default(),
                node_rects: &mut Default::default(),
                node_id: node,
                ongoing_drag: None,
                selected: false,
                pan: away.to_vec2(),
            }
            .show(&life_size, &mut offscreen, user_state);
        }
        if let Some(rect) = annotation_ui::module_rect(&ctx, node) {
            sizes.insert(node, rect.size());
        }
    }

    ctx.options_mut(|o| o.warn_on_id_clash = warn_on_id_clash);
    user_state.zoom = zoom;
    for id in faces {
        user_state.group_faces.remove(&id);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{create_editor_state, groups, GroupId};
    use crate::persistence::{stage_patch, EXAMPLES};

    /// An example opened as the app opens it, on its top level.
    fn open(file_name: &str) -> (SynthGraphEditorState, SynthGraphState) {
        let example = EXAMPLES.iter().find(|e| e.file_name == file_name).unwrap();
        let mut staged = stage_patch(&example.patch().unwrap()).unwrap();
        let mut editor = create_editor_state();
        let mut user_state = SynthGraphState::new();
        editor.graph = std::mem::take(&mut staged.graph);
        let placed = staged.parts.iter().map(|p| (p.graph_id, p.position)).chain(staged.nodes.iter().map(|n| (n.graph_id, n.position)));
        for (node, (x, y)) in placed {
            editor.node_positions.insert(node, egui::pos2(x, y));
            editor.node_order.push(node);
            user_state.allocate_engine_node_id(node);
        }
        (editor, user_state)
    }

    /// Shows `level`, as the app does before drawing it.
    fn show_level(editor: &SynthGraphEditorState, user_state: &mut SynthGraphState, sizes: &HashMap<NodeId, Vec2>, level: Option<GroupId>) {
        let graph = &editor.graph;
        let index = GroupIndex::of(graph);
        user_state.level = level;
        user_state.hidden = graph.nodes.iter().filter(|(_, n)| n.user_data.parent != level).map(|(id, _)| id).collect();
        let positions = &editor.node_positions;
        user_state.group_faces = groups::level_nodes(graph, level)
            .into_iter()
            .filter_map(|n| graph[n].user_data.kind.group())
            .map(|id| (id, face_of(graph, |n| positions.get(n).copied(), sizes, 1.0, &index, id)))
            .collect();
    }

    /// A context dressed as the app's.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::app::theme::install_fonts(&ctx);
        crate::app::theme::apply_theme(&ctx);
        ctx
    }

    fn frame(ctx: &egui::Context, time: f64, mut add_contents: impl FnMut(&mut egui::Ui)) {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0));
        let input = egui::RawInput { time: Some(time), screen_rect: Some(screen), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| add_contents(ui));
        });
    }

    /// Every module inside every group of `file_name` is measured before
    /// it's shown, as big as the editor then draws it.
    fn measured_as_drawn(file_name: &str) {
        let (mut editor, mut user_state) = open(file_name);
        let ctx = context();
        let mut sizes = HashMap::new();
        show_level(&editor, &mut user_state, &sizes, None);
        let faces: Vec<GroupId> = user_state.group_faces.keys().copied().collect();
        frame(&ctx, 0.0, |ui| assert!(measure(&mut editor, &mut user_state, &mut sizes, ui)));
        assert!(user_state.hidden.iter().all(|n| sizes.contains_key(n)), "every hidden module measured");
        assert_eq!(user_state.group_faces.len(), faces.len(), "only the level's own faces are left");
        // Measured once: nothing's left for the next frame
        frame(&ctx, 0.1, |ui| assert!(!measure(&mut editor, &mut user_state, &mut sizes, ui)));

        let index = GroupIndex::of(&editor.graph);
        let mut time = 1.0;
        for level in index.ids() {
            show_level(&editor, &mut user_state, &sizes, Some(level));
            // Drawn until the editor's outlines have settled
            for _ in 0..4 {
                time += 0.1;
                frame(&ctx, time, |ui| {
                    let _ = editor.draw_graph_editor(ui, crate::graph::AllNodeTemplates, &mut user_state, Vec::default());
                });
            }
            for node in groups::level_nodes(&editor.graph, Some(level)) {
                let drawn = annotation_ui::module_rect(&ctx, node).unwrap().size();
                let name = &editor.graph[node].label;
                assert!((drawn - sizes[&node]).length() < 0.5, "{name} drawn {drawn:?}, measured {:?}", sizes[&node]);
            }
        }
    }

    #[test]
    fn front_porch_is_measured_as_drawn() {
        measured_as_drawn("front-porch.json");
    }

    #[test]
    fn canopy_is_measured_as_drawn() {
        // Groups inside groups: their faces show their insides measured
        measured_as_drawn("canopy.json");
    }

    #[test]
    fn modules_are_taller_than_the_nominal_card() {
        let (mut editor, mut user_state) = open("front-porch.json");
        let ctx = context();
        let mut sizes = HashMap::new();
        show_level(&editor, &mut user_state, &sizes, None);
        frame(&ctx, 0.0, |ui| {
            measure(&mut editor, &mut user_state, &mut sizes, ui);
        });
        assert!(sizes.values().any(|s| s.y > groups::NOMINAL_NODE_SIZE.y * 1.5));
    }
}
