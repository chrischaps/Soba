//! Groups in the editor: making and taking them apart, naming them, going
//! inside and back out, and the knobs pinned to their faces.
//!
//! The editor shows one level of the patch at a time: the top, or the
//! inside of one group. [`SynthApp::prepare_level`] works out, before each
//! frame is drawn, which nodes that hides, what the groups on show have on
//! their faces, and which module output each group jack carries, so cables
//! from jacks show the signal flowing through them.

use eframe::egui::{self, vec2, FontId, RichText};
use egui_node_graph2::NodeId;

use crate::graph::groups::{self, Face, GroupIndex, JackAction, JackEdit, Preview, Renaming, Side};
use crate::graph::{annotation_ui, GroupId};
use crate::persistence::{capture_level, kit_pads, Patch};
use super::super::editing;
use super::super::library::{self, SavedModule};
use super::super::recording;
use super::super::shortcuts;
use super::super::theme;
use super::framing::View;
use super::SynthApp;

/// The miniature on a group's face is laid out at this zoom, so it looks
/// the same however far the view is zoomed.
const PREVIEW_ZOOM: f32 = 1.0;

/// Space kept around a level's nodes when the view frames them.
pub(super) const FRAME_MARGIN: f32 = 40.0;
/// Room left at the top of the canvas for the breadcrumb trail.
pub(super) const TRAIL_HEIGHT: f32 = 56.0;

impl SynthApp {
    /// Works out what this frame shows: the nodes on the current level, the
    /// faces of the groups on it, and what their jacks carry.
    pub(super) fn prepare_level(&mut self) {
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        // A level that's gone (undone, or deleted) leaves for the top
        if self.user_state.level.is_some_and(|level| index.node(level).is_none()) {
            self.user_state.level = None;
            self.level_trail.clear();
        }
        let level = self.user_state.level;

        self.user_state.hidden =
            graph.nodes.iter().filter(|(_, n)| n.user_data.parent != level).map(|(id, _)| id).collect();
        let hidden = &self.user_state.hidden;
        self.graph_state.selected_nodes.retain(|n| !hidden.contains(n));

        self.user_state.cables_from.rebuild(graph);

        // A jack's cable shows what's plugged in behind it
        self.user_state.output_aliases.clear();
        for (node_id, node) in graph.nodes.iter().filter(|(_, n)| !n.user_data.is_module()) {
            for (k, (_, output)) in node.outputs.iter().enumerate() {
                let Some(source) = groups::leaf_source(graph, &index, *output) else { continue };
                let from = graph.get_output(source).node;
                if let (Some(engine_id), Some(output_index)) =
                    (self.user_state.get_engine_node_id(from), graph.get_output_index(source))
                {
                    self.user_state.output_aliases.insert((node_id, k), (engine_id, output_index));
                }
            }
        }

        // What each port on show can do with groups' jacks, for its menu
        let shown = graph.nodes.iter().filter(|(id, _)| !hidden.contains(id));
        self.user_state.jack_actions = shown
            .filter(|(_, node)| level.is_some() || !node.user_data.is_module())
            .flat_map(|(node_id, node)| {
                let ports = |side: Side, names: Vec<&String>| {
                    let actions: Vec<(String, Vec<JackAction>)> = names
                        .into_iter()
                        .map(|name| (name.clone(), groups::jack_actions(graph, &index, node_id, side, name)))
                        .filter(|(_, actions)| !actions.is_empty())
                        .collect();
                    ((node_id, side), actions)
                };
                [
                    ports(Side::Input, node.inputs.iter().map(|(name, _)| name).collect()),
                    ports(Side::Output, node.outputs.iter().map(|(name, _)| name).collect()),
                ]
            })
            .filter(|(_, actions)| !actions.is_empty())
            .collect();

        // The faces of the groups on show
        let positions = &self.graph_state.node_positions;
        let sizes = &self.node_sizes;
        let zoom = self.graph_state.pan_zoom.zoom;
        self.user_state.group_faces = graph
            .nodes
            .iter()
            .filter(|(id, _)| !hidden.contains(id))
            .filter_map(|(_, node)| node.user_data.kind.group())
            .map(|id| (id, face_of(graph, |n| positions.get(n).copied(), sizes, zoom, &index, id)))
            .collect();

        if self.user_state.renaming.as_ref().is_some_and(|r| index.node(r.group).is_none()) {
            self.user_state.renaming = None;
        }
        if self.user_state.renaming.is_none() {
            self.naming_group = None;
        }

        // A level framed before its nodes (and frames and notes) had ever
        // been drawn is framed again once they have
        if self.reframe_level && self.level_bounds(level).is_none_or(|(_, measured)| measured) {
            self.frame_level();
        }
    }

    /// Notes how big each node on show was drawn, unzoomed, for framing
    /// levels and drawing groups' miniatures true to shape.
    ///
    /// A node's rect is sensed before it's drawn, at the size it was last
    /// frame. While the zoom moves that size is at the last frame's zoom,
    /// so only nodes never measured are measured then: measuring the rest
    /// would swell them as the view zooms out, and a glide framing them
    /// would chase them further out.
    pub(super) fn remember_node_sizes(&mut self, ctx: &egui::Context) {
        let zoom = self.graph_state.pan_zoom.zoom;
        let steady = zoom == self.sized_at_zoom;
        self.sized_at_zoom = zoom;
        for &node in &self.graph_state.node_order {
            if self.user_state.hidden.contains(&node) || (!steady && self.node_sizes.contains_key(&node)) {
                continue;
            }
            if let Some(rect) = annotation_ui::module_rect(ctx, node) {
                self.node_sizes.insert(node, rect.size() / zoom);
            }
        }
        let graph = &self.graph_state.graph;
        self.node_sizes.retain(|node, _| graph.nodes.contains_key(*node));
    }

    /// Groups the selected modules (and groups), and opens the new group's
    /// name for typing. Naming it is part of the same undo step.
    pub(super) fn group_selection(&mut self, ctx: &egui::Context, nodes: &[NodeId]) {
        let graph = &self.graph_state.graph;
        let nodes: Vec<NodeId> = nodes
            .iter()
            .copied()
            .filter(|n| graph.nodes.get(*n).is_some_and(|node| !node.user_data.kind.is_proxy()))
            .collect();
        if nodes.is_empty() {
            self.status_message = Some(shortcuts::keys(ctx, "Select modules to group them (Ctrl+G)").into_owned());
            return;
        }
        let what = editing::describe_modules(&self.graph_state, &nodes);

        // Where the selection is drawn, for placing Inputs and Outputs either side
        let zoom = self.graph_state.pan_zoom.zoom;
        let drawn = nodes
            .iter()
            .filter_map(|&n| annotation_ui::module_rect(ctx, n))
            .map(|r| egui::Rect::from_min_max(self.screen_to_node(r.min), self.screen_to_node(r.max)));
        let placed = nodes.iter().filter_map(|&n| {
            let at = *self.graph_state.node_positions.get(n)?;
            Some(egui::Rect::from_min_size(at, groups::NOMINAL_NODE_SIZE * zoom))
        });
        let Some(bounds) = drawn.chain(placed).reduce(|a, b| a.union(b)) else { return };

        let id = self.user_state.allocate_group_id();
        let Some(new) = groups::group(&mut self.graph_state, &nodes, id, "Group", bounds) else {
            return;
        };
        self.select(vec![new.node], Vec::new());
        self.user_state.renaming = Some(Renaming { group: id, text: "Group".to_string(), fresh: true });
        self.naming_group = Some(id);
        self.history.name_next(format!("Group {what}"));
        self.status_message = Some(format!("Grouped {what}: name the group, then press Enter"));
    }

    /// Takes groups apart onto the level they sit on, their modules
    /// selected. With no group among `nodes`, inside a group, it takes that
    /// group apart and shows the level it was on.
    pub(super) fn ungroup(&mut self, ctx: &egui::Context, nodes: &[NodeId]) {
        let graph = &self.graph_state.graph;
        let mut targets: Vec<NodeId> =
            nodes.iter().copied().filter(|n| graph.nodes.get(*n).is_some_and(|n| n.user_data.kind.group().is_some())).collect();
        if targets.is_empty() {
            if let Some(level) = self.user_state.level {
                let index = GroupIndex::of(graph);
                if let Some(node) = index.node(level) {
                    self.leave_group();
                    targets.push(node);
                }
            }
        }
        if targets.is_empty() {
            self.status_message = Some(shortcuts::keys(ctx, "Select a group to ungroup it (Ctrl+Alt+G)").into_owned());
            return;
        }
        let what = editing::describe_modules(&self.graph_state, &targets);
        let mut out = Vec::new();
        for node in targets {
            out.extend(groups::ungroup(&mut self.graph_state, node).unwrap_or_default());
        }
        self.select(out, Vec::new());
        self.history.name_next(format!("Ungroup {}", what.trim_start_matches("group ")));
        self.status_message = Some(format!("Ungrouped {}", what.trim_start_matches("group ")));
    }

    /// Opens a group's name for typing.
    pub(super) fn start_rename(&mut self, node_id: NodeId) {
        let Some(node) = self.graph_state.graph.nodes.get(node_id) else { return };
        let Some(group) = node.user_data.kind.group() else { return };
        let text = node.user_data.display_name.clone();
        self.user_state.renaming = Some(Renaming { group, text, fresh: true });
    }

    /// Names a group, as typed.
    pub(super) fn rename_group(&mut self, node_id: NodeId, name: &str) {
        let Some(group) = self.graph_state.graph.nodes.get(node_id).and_then(|n| n.user_data.kind.group()) else {
            return;
        };
        groups::rename(&mut self.graph_state.graph, node_id, name);
        if self.naming_group == Some(group) {
            self.history.name_next(format!("Group {name}"));
            self.status_message = Some(format!("Grouped as {name}: double-click it to go inside"));
        } else {
            self.history.name_next(format!("Rename group {name}"));
        }
    }

    /// Shows a module's knob, dropdown or toggle on the faces of the groups
    /// around it, `levels` groups up, or with 0, on none.
    pub(super) fn pin_knob(&mut self, node_id: NodeId, param_name: &str, levels: u8) {
        let graph = &mut self.graph_state.graph;
        if !graph.nodes.contains_key(node_id) {
            return;
        }
        let label = groups::control_label(graph, node_id, param_name);
        let data = &mut graph[node_id].user_data;
        let before = data.pin_levels(param_name);
        if levels == 0 {
            // Off every face, it has no use for a face's name
            data.pins.remove(param_name);
            data.face_labels.remove(param_name);
        } else {
            data.pins.insert(param_name.to_string(), levels);
        }
        let what = format!("{} {}", data.display_name, label);
        let (verb, done) = if levels > before { ("Pin", "on its group's face") } else { ("Unpin", "off the group's face") };
        self.history.name_next(format!("{verb} {what}"));
        self.status_message = Some(format!("{what} {done}"));
    }

    /// Calls a pinned control `name` on the groups' faces. A blank name, or
    /// its module's own short name, takes the face's name off it.
    pub(super) fn name_face_control(&mut self, node_id: NodeId, param_name: &str, name: &str) {
        let graph = &mut self.graph_state.graph;
        let Some((was, now)) = groups::name_face_control(graph, node_id, param_name, name) else { return };
        let own = groups::control_label(graph, node_id, param_name);
        let module = &graph[node_id].user_data.display_name;
        self.history.name_next(format!("Rename {was} to {now} on face"));
        self.status_message = Some(if now == own {
            format!("{module} {own} goes by its own name on the face again")
        } else {
            format!("{module} {own} reads {now} on the face")
        });
    }

    /// Does what a port's menu chose with groups' jacks: shows the port on
    /// the group it's in as a new jack, unplugs it from the group's jack, or
    /// renames, moves or takes away the jack it is.
    pub(super) fn edit_jack(&mut self, node_id: NodeId, side: Side, port: &str, edit: JackEdit) {
        let graph = &mut self.graph_state.graph;
        let group_name = |graph: &crate::graph::SynthGraph, id| {
            GroupIndex::of(graph).node(id).map_or_else(String::new, |n| groups::name(graph, n).to_string())
        };
        let (undo, status) = match edit {
            JackEdit::Show => {
                let Some((id, jack)) = groups::show_port(graph, node_id, side, port) else { return };
                let group = group_name(graph, id);
                (format!("Add jack {jack} to {group}"), format!("{group} has a new jack, {jack}"))
            }
            JackEdit::Hide => {
                let Some((id, jacks)) = groups::hide_port(graph, node_id, side, port) else { return };
                let group = group_name(graph, id);
                let module = graph.nodes.get(node_id).map_or("", |n| n.user_data.display_name.as_str());
                match jacks.as_slice() {
                    [(jack, true)] => (format!("Remove jack {jack} from {group}"), format!("{group} no longer has a {jack} jack")),
                    [(jack, false)] => (
                        format!("Unplug {module} {port} from {group}"),
                        format!("{module} {port} is unplugged from {group}'s {jack} jack, which still feeds the rest"),
                    ),
                    _ => (format!("Remove jacks from {group}"), format!("{module} {port} is no longer shown on {group}")),
                }
            }
            JackEdit::Remove => {
                let Some((id, jack)) = groups::remove_port_jack(graph, node_id, side, port) else { return };
                let group = group_name(graph, id);
                (format!("Remove jack {jack} from {group}"), format!("{group} no longer has a {jack} jack"))
            }
            JackEdit::Rename(name) => {
                let Some((id, old, new)) = groups::rename_jack(graph, node_id, side, port, &name) else { return };
                let group = group_name(graph, id);
                let numbered = if new != name.trim() { format!(" ({} is taken)", name.trim()) } else { String::new() };
                (format!("Rename jack {old} to {new}"), format!("{group}'s {old} jack is now {new}{numbered}"))
            }
            JackEdit::Move { up } => {
                let Some((id, jack)) = groups::move_jack(graph, node_id, side, port, up) else { return };
                let group = group_name(graph, id);
                let way = if up { "up" } else { "down" };
                (format!("Move jack {jack} {way}"), format!("{group}'s {jack} jack moved {way}"))
            }
        };
        self.history.name_next(undo);
        self.status_message = Some(status);
    }

    /// Shows the inside of a group, straight away, all of it in view.
    pub(super) fn enter_group(&mut self, node_id: NodeId) {
        let Some(id) = self.graph_state.graph.nodes.get(node_id).and_then(|n| n.user_data.kind.group()) else {
            return;
        };
        // The view outside, to come back out to
        let outside = self.level_view.unwrap_or_else(|| self.view());
        self.level_trail.push((id, outside));
        self.show_level(Some(id));
        self.frame_level();
        let name = groups::name(&self.graph_state.graph, node_id).to_string();
        self.status_message = Some(format!("Inside {name}: Esc to go back out"));
    }

    /// Goes back out of the group being shown, straight away, to the view
    /// it was entered from. Returns whether there was a group to leave.
    pub(super) fn leave_group(&mut self) -> bool {
        self.land_dive();
        self.step_out()
    }

    /// Steps out as [`Self::leave_group`] does, leaving any dive be.
    pub(super) fn step_out(&mut self) -> bool {
        let Some(level) = self.user_state.level else { return false };
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        let parent = index.parent_of(graph, level);
        let outside = self.view_entered_from(level);
        if self.level_trail.last().is_some_and(|&(id, _)| id == level) {
            self.level_trail.pop();
        }
        self.show_level(parent);
        match outside {
            Some(view) => self.show_view(view),
            None => self.frame_level(),
        }
        if let Some(node) = index.node(level) {
            self.graph_state.selected_nodes = vec![node];
            let name = groups::name(&self.graph_state.graph, node).to_string();
            self.status_message = Some(format!("Back out of {name}"));
        }
        true
    }

    /// The view the group `level` was entered from, if the view went into it.
    fn view_entered_from(&self, level: GroupId) -> Option<View> {
        match self.level_trail.last() {
            Some(&(id, view)) if id == level => Some(view),
            _ => None,
        }
    }

    /// The view stepping out of the group `level` will leave.
    pub(super) fn view_out_of(&self, level: GroupId) -> Option<View> {
        let graph = &self.graph_state.graph;
        self.view_entered_from(level).or_else(|| self.framed_view(GroupIndex::of(graph).parent_of(graph, level)))
    }

    /// Shows a level: back out to it if it's one the view is inside,
    /// otherwise straight there.
    pub(super) fn go_to_level(&mut self, target: Option<GroupId>) {
        self.land_dive();
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        let Some(path) = index.path(graph, self.user_state.level) else { return };
        if target.is_none() || target.is_some_and(|t| path.contains(&t)) {
            while self.user_state.level != target && self.step_out() {}
            return;
        }
        if index.path(graph, target).is_none() {
            return;
        }
        // No view to come back out to: the view frames each level
        self.level_trail.clear();
        self.show_level(target);
        self.frame_level();
    }

    fn show_level(&mut self, level: Option<GroupId>) {
        self.user_state.level = level;
        self.graph_state.selected_nodes.clear();
        self.user_state.annotations.selected.clear();
        // Where frames and notes were drawn was on the level left, in its view
        self.user_state.annotations.drawn.clear();
        self.user_state.renaming = None;
        self.user_state.context_menu_pos = None;
        self.graph_state.connection_in_progress = None;
        self.prepare_level();
    }

    /// Fits the current level to the view (never closer than 1:1), its
    /// frames and notes as last drawn included: for a patch opened on a
    /// small screen, where the signal path would otherwise run off the edge.
    pub(super) fn fit_level(&mut self, ui: &egui::Ui) {
        self.frame_level();
        self.apply_level_view(ui);
    }

    /// Frames everything on the current level, as F does with nothing
    /// selected, once the frame is drawn.
    fn frame_level(&mut self) {
        // Nodes never drawn yet are taken at a nominal size, frames and notes
        // never drawn are left out, and the level is framed again once they are
        self.reframe_level = self.level_bounds(self.user_state.level).is_some_and(|(_, measured)| !measured);
        if let Some(view) = self.framed_view(self.user_state.level) {
            self.show_view(view);
        }
    }

    /// Shows the level on show from `view` rather than framed whole, and
    /// leaves it there once its modules are measured.
    pub(super) fn land_level_on(&mut self, view: View) {
        self.reframe_level = false;
        self.show_view(view);
    }

    /// Puts the view there as the next frame is drawn: zooming takes the
    /// editor's [`egui::Ui`].
    fn show_view(&mut self, view: View) {
        self.level_view = Some(view);
        // A glide would take the view somewhere else
        self.glide = None;
    }

    /// Puts the view where stepping between levels left it to go.
    pub(super) fn apply_level_view(&mut self, ui: &egui::Ui) {
        if let Some(view) = self.level_view.take() {
            self.set_view(ui, view);
        }
    }

    /// The miniature of a group's insides, as its face shows it.
    pub(super) fn preview_of(&self, id: GroupId) -> Preview {
        let state = &self.graph_state;
        preview_of(&state.graph, |n| state.node_positions.get(n).copied(), &self.node_sizes, state.pan_zoom.zoom, id)
    }

    /// Ctrl+G groups the selection, Ctrl+Alt+G takes groups apart, F2
    /// renames the selected group, Tab opens it and Escape comes back out.
    /// Tab with no group selected is left for the quick-add palette.
    pub(super) fn handle_group_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let selected_group = match self.graph_state.selected_nodes.as_slice() {
            [one] => self.graph_state.graph.nodes.get(*one).and_then(|n| n.user_data.kind.group()).map(|_| *one),
            _ => None,
        };
        let can_leave = self.user_state.level.is_some()
            && self.user_state.context_menu_pos.is_none()
            && !self.is_midi_learning()
            && self.user_state.annotations.editing.is_none()
            // Escape closes an open popup (a step's piano) first
            && !egui::Popup::is_any_open(ctx);
        let (ungroup, group, rename, enter, leave) = ctx.input_mut(|i| {
            // Ctrl+G alone would also match Ctrl+Alt+G, so that goes first
            let ungroup = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::ALT, Key::G));
            let group = !ungroup && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::G));
            let rename = selected_group.is_some() && i.consume_key(Modifiers::NONE, Key::F2);
            let enter = selected_group.is_some() && i.consume_key(Modifiers::NONE, Key::Tab);
            let leave = can_leave && i.consume_key(Modifiers::NONE, Key::Escape);
            (ungroup, group, rename, enter, leave)
        });
        let selection = self.graph_state.selected_nodes.clone();
        if ungroup {
            self.ungroup(ctx, &selection);
        } else if group {
            self.group_selection(ctx, &selection);
        }
        if let (Some(node), true) = (selected_group, rename) {
            self.start_rename(node);
        }
        if let (Some(node), true) = (selected_group, enter) {
            self.dive_into(ctx, node);
            // egui also walks its keyboard focus on to a toolbar button with
            // Tab, before anything can stop it: let it go next frame
            self.release_tab_focus = true;
        }
        if leave {
            self.dive_out(ctx);
        }
    }

    /// Removes what was inside a group whose node has just been deleted.
    pub(super) fn delete_group_contents(&mut self, id: GroupId) {
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        let inside = groups::descendants(graph, &index, id);
        for cmd in editing::delete_modules(&mut self.graph_state, &mut self.user_state, &inside) {
            self.send_command(cmd);
        }
    }

    /// Saves a group to My Modules under its name, ready to add to any
    /// patch. One already saved under that name is updated.
    pub(super) fn save_to_library(&mut self, node_id: NodeId) {
        let graph = &self.graph_state.graph;
        if graph.nodes.get(node_id).and_then(|n| n.user_data.kind.group()).is_none() {
            return;
        }
        let name = groups::name(graph, node_id).to_string();
        // In patch space, from the group's own corner
        let zoom = self.graph_state.pan_zoom.zoom;
        let positions = &self.graph_state.node_positions;
        let corner = positions.get(node_id).map_or(egui::Pos2::ZERO, |&p| self.history.to_patch(p, zoom));
        let position = |n: NodeId| {
            let at = positions.get(n).map_or(corner, |&p| self.history.to_patch(p, zoom)) - corner;
            (at.x, at.y)
        };
        let engine_id = |n: NodeId| self.user_state.get_engine_node_id(n);
        let level = capture_level(graph, &[node_id], None, &engine_id, &position);
        let mut patch = Patch::new(name.clone());
        patch.groups = level.groups;
        patch.version = patch.required_version();
        match library::save(&patch) {
            Ok((_, false)) => self.status_message = Some(format!("Saved {name} to My Modules: it's in the add menu and the palette")),
            Ok((_, true)) => self.status_message = Some(format!("Updated {name} in My Modules")),
            Err(e) => self.status_error(format!("Couldn't save {name} to My Modules: {e}")),
        }
        self.my_modules = library::list();
    }

    /// Adds a group from My Modules or the Library, its corner at a point
    /// on screen.
    pub(super) fn add_saved_module(&mut self, saved: &SavedModule, screen: egui::Pos2) {
        let patch = match saved.load() {
            Ok(patch) => patch,
            Err(e) => {
                self.status_error(format!("Couldn't add {}: {e}", saved.name));
                return;
            }
        };
        if self.add_patch(&patch, &saved.name, screen) {
            self.status_message = Some(format!("Added {} from {}", saved.name, saved.from_where()));
        }
    }

    /// Adds a group of Pads built from the kit profile `name` (#469), each
    /// on its part's note and channel, with a MIDI In hearing the device
    /// where it's open.
    pub(super) fn add_pads_for(&mut self, name: &str, screen: egui::Pos2) {
        let profiles = crate::hardware::known();
        let Some(known) = profiles.find(name) else {
            self.status_error(format!("Couldn't add Pads for {name}: its profile is gone"));
            return;
        };
        let open = self.midi.open_names();
        let device = kit_pads::device_for(&known.profile, &open);
        let patch = kit_pads::pads_for(&known.profile, device);
        if self.add_patch(&patch, &format!("Pads for {name}"), screen) {
            let heard = device.map_or_else(|| format!(": tick the {name} under MIDI In, then choose it on the MIDI In"), |device| format!(", hearing {device}"));
            self.status_message = Some(format!("Added {} Pads for the {name}{heard}", known.profile.parts.len()));
        }
    }

    /// Pastes `patch` with its corner at a point on screen, as one undo
    /// step called "Add `name`". Whether it went in.
    fn add_patch(&mut self, patch: &Patch, name: &str, screen: egui::Pos2) -> bool {
        let at = self.screen_to_node(screen);
        match editing::paste(&mut self.graph_state, &mut self.user_state, patch, at) {
            Ok(pasted) => {
                if !pasted.warnings.is_empty() {
                    self.load_warnings = pasted.warnings.clone();
                }
                self.finish_paste(pasted, &format!("Add {name}"));
                true
            }
            Err(e) => {
                self.status_error(format!("Couldn't add {name}: {e}"));
                false
            }
        }
    }

    /// Shows the My Modules folder, making it if it isn't there yet.
    pub(super) fn open_library_folder(&mut self) {
        let folder = library::folder();
        if let Err(e) = std::fs::create_dir_all(&folder).and_then(|()| recording::open_folder(&folder)) {
            self.status_error(format!("Couldn't open {}: {e}", folder.display()));
        }
    }

    /// Whether a selection is on the top level, where frames and notes go.
    pub(super) fn frames_allowed(&self) -> bool {
        self.user_state.level.is_none()
    }

    /// The trail of levels shown at the top of the canvas inside a group,
    /// "Patch › Voice › Tone": click one to go back out to it.
    pub(super) fn draw_breadcrumbs(&mut self, ctx: &egui::Context) {
        let Some(level) = self.user_state.level else { return };
        let graph = &self.graph_state.graph;
        let index = GroupIndex::of(graph);
        let Some(path) = index.path(graph, Some(level)) else { return };
        let mut crumbs: Vec<(Option<GroupId>, String)> = vec![(None, self.document.title())];
        crumbs.extend(path.into_iter().map(|id| (Some(id), index.node(id).map_or("Group", |n| groups::name(graph, n)).to_string())));

        let rose = theme::module::GROUP;
        let mut go_to = None;
        egui::Area::new(egui::Id::unique("group_breadcrumbs"))
            .fixed_pos(self.editor_rect.left_top() + vec2(14.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(theme::background::PANEL.gamma_multiply(0.96))
                    .stroke(egui::Stroke::new(1.0_f32, rose.gamma_multiply(0.55)))
                    .corner_radius(16.0)
                    .inner_margin(egui::Margin::symmetric(14, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let last = crumbs.len() - 1;
                            for (i, (target, name)) in crumbs.iter().enumerate() {
                                if i > 0 {
                                    ui.label(RichText::new("›").color(theme::text::DISABLED).size(15.0));
                                }
                                if i == last {
                                    ui.label(RichText::new(name).color(rose).strong().size(14.0));
                                } else {
                                    let crumb = egui::Button::new(RichText::new(name).color(theme::text::SECONDARY).size(14.0))
                                        .frame(false);
                                    if ui.add(crumb).on_hover_text("Back out to here").clicked() {
                                        go_to = Some(*target);
                                    }
                                }
                            }
                            ui.add_space(6.0);
                            ui.label(RichText::new("Esc to go back out").color(theme::text::QUIET).small());
                        });
                    });
            });
        if let Some(target) = go_to {
            self.go_to_level(target);
        }
    }

    /// Inside a group, the canvas says so: the group's name set large and
    /// faint in the corner, and a rose edge around the view.
    pub(super) fn draw_level_backdrop(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(level) = self.user_state.level else { return };
        let graph = &self.graph_state.graph;
        let Some(node) = GroupIndex::of(graph).node(level) else { return };
        let rose = theme::module::GROUP;
        painter.text(
            rect.right_bottom() + vec2(-32.0, -22.0),
            egui::Align2::RIGHT_BOTTOM,
            groups::name(graph, node),
            FontId::new(72.0, egui::FontFamily::Name(theme::TITLE_FAMILY.into())),
            rose.gamma_multiply(0.07),
        );
        painter.rect_stroke(rect.shrink(1.0), 0.0, egui::Stroke::new(2.0_f32, rose.gamma_multiply(0.35)), egui::StrokeKind::Middle);
    }
}

/// The miniature of a group's insides as its face shows it, from where its
/// nodes are in the editor (`position`), how big they were last drawn, and
/// the zoom.
fn preview_of(
    graph: &crate::graph::SynthGraph,
    position: impl Fn(NodeId) -> Option<egui::Pos2>,
    sizes: &std::collections::HashMap<NodeId, egui::Vec2>,
    zoom: f32,
    id: GroupId,
) -> Preview {
    let unzoomed = |n: NodeId| position(n).map(|p| (p.to_vec2() / zoom * PREVIEW_ZOOM).to_pos2());
    groups::preview(graph, unzoomed, |n| sizes.get(&n).copied(), id, PREVIEW_ZOOM)
}

/// What a group's node shows on its face: its miniature and the controls
/// pinned to it.
pub(super) fn face_of(
    graph: &crate::graph::SynthGraph,
    position: impl Fn(NodeId) -> Option<egui::Pos2> + Copy,
    sizes: &std::collections::HashMap<NodeId, egui::Vec2>,
    zoom: f32,
    index: &GroupIndex,
    id: GroupId,
) -> Face {
    Face {
        preview: preview_of(graph, position, sizes, zoom, id),
        controls: groups::face_controls(graph, position, index, id),
        learnable: crate::graph::learn_walk::count(graph, id),
    }
}
