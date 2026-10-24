//! The canvas: the node graph editor, what its modules ask for as they're
//! drawn, their right-click menus, and the add menu and palette over it.

use super::*;

/// What a module's right-click menu asked for, handled once the graph is drawn.
enum NodeMenuAction {
    Select(egui_node_graph2::NodeId),
    Duplicate(egui_node_graph2::NodeId),
    Copy(egui_node_graph2::NodeId),
    Reset(egui_node_graph2::NodeId),
    Delete(egui_node_graph2::NodeId),
    Group(egui_node_graph2::NodeId),
    Ungroup(egui_node_graph2::NodeId),
    Enter(egui_node_graph2::NodeId),
    StartRename(egui_node_graph2::NodeId),
    Rename(egui_node_graph2::NodeId, String),
    Pin(egui_node_graph2::NodeId, String, u8),
    NameFace(egui_node_graph2::NodeId, String, String),
    /// A port's menu chose what to do with groups' jacks.
    Jack(egui_node_graph2::NodeId, crate::graph::groups::Side, String, crate::graph::groups::JackEdit),
    SaveToLibrary(egui_node_graph2::NodeId),
    OpenSample(egui_node_graph2::NodeId),
    RecordInto(egui_node_graph2::NodeId, crate::graph::resample::Source, u32),
}

/// What drawing the graph asked for, acted on once it's drawn (to avoid
/// borrow issues).
#[derive(Default)]
struct CanvasAsks {
    /// Connections to remove (validated after drawing)
    invalid_connections: Vec<(egui_node_graph2::OutputId, egui_node_graph2::InputId)>,
    /// The cables before one was dropped on a full input, to put back if it's refused
    before_drop: Option<crate::graph::refused_cable::Before>,
    /// Commands to send to the engine
    commands: Vec<EngineCommand>,
    /// Nodes whose bypass switch was clicked
    bypass_toggles: Vec<egui_node_graph2::NodeId>,
    /// What modules' right-click menus asked for
    node_menu_actions: Vec<NodeMenuAction>,
    /// Groups whose node was closed, whose insides go too
    deleted_groups: Vec<GroupId>,
    /// Whether the pointer was over the editor
    cursor_in_editor: bool,
}

impl SynthApp {
    /// Draw the main content area with the node graph editor
    pub(super) fn draw_main_area(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let mut asks = CanvasAsks::default();
        // What this frame shows: the top of the patch, or a group's inside
        self.prepare_level();

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.draw_canvas(ui, &mut asks));

        self.open_add_menu(ctx, asks.cursor_in_editor);
        self.show_quick_add(ctx);

        // Send collected commands
        for cmd in asks.commands {
            self.send_command(cmd);
        }
        for node_id in asks.bypass_toggles {
            self.toggle_bypass(node_id);
        }
        for action in asks.node_menu_actions {
            self.handle_node_menu(ctx, action);
        }
        for id in asks.deleted_groups {
            self.delete_group_contents(id);
        }
        self.draw_breadcrumbs(ctx);

        // Remove invalid connections outside the UI closure, putting back any they replaced
        for (output, input) in asks.invalid_connections {
            crate::graph::refused_cable::refuse(&mut self.graph_state.graph, output, input, asks.before_drop.as_ref());
        }
    }

    /// The grid, the graph and whatever moves the view this frame: a step
    /// between levels, a dive, F's glide, a fit, gestures and a capture's camera.
    fn draw_canvas(&mut self, ui: &mut egui::Ui, asks: &mut CanvasAsks) {
        let ctx = &ui.ctx().clone();
        // Store editor rect for coordinate conversion
        let editor_rect = ui.available_rect_before_wrap();
        self.editor_rect = editor_rect;

        // Reset widget context menu flag before drawing
        self.user_state.widget_context_menu_open = false;

        // Modules inside groups are measured before they're first
        // shown, so faces, framing and dives are true to them
        if unseen::measure(&mut self.graph_state, &mut self.user_state, &mut self.node_sizes, ui) {
            self.prepare_level();
        }

        // A step into or out of a group, then F's glide, first, so
        // all of this frame is drawn where they've got to
        self.apply_level_view(ui);
        let dive = self.step_dive(ui);
        self.step_glide(ui);

        // Update zoom for widget scaling, and where frames and notes go
        self.user_state.zoom = self.graph_state.pan_zoom.zoom;
        self.user_state.view_origin = self.history.view_origin();

        // The grid sits under the patch and moves with it, as a dive
        // zooms it too
        let layer = ui.layer_id();
        let next_shape = || ctx.graphics_mut(|g| g.entry(layer).next_idx());
        let grid_start = next_shape();
        let grid_origin = editor_rect.min + self.graph_state.pan_zoom.pan + self.history.view_origin();
        let canvas = Self::dive_canvas(dive.as_ref(), editor_rect);
        let mut grid_painter = ui.painter().clone();
        grid_painter.set_clip_rect(canvas);
        theme::draw_grid_background(&grid_painter, canvas, grid_origin, self.graph_state.pan_zoom.zoom);
        let grid_end = next_shape();
        self.draw_level_backdrop(ui.painter(), editor_rect);

        // The view the copy before this one had, after an update
        #[cfg(not(target_arch = "wasm32"))]
        self.apply_pending_view(ui, editor_rect);

        // Fitted again each frame for a few: zoomed out, modules that
        // were off screen are drawn and measured, and may need more room.
        // Before the zoom below is noted: the fit follows its own zoom
        if self.fit_pending > 0 {
            self.fit_pending -= 1;
            self.fit_level(ui);
        }

        // Draw the node graph editor
        let (zoom_before, pan_before) = (self.graph_state.pan_zoom.zoom, self.graph_state.pan_zoom.pan);
        self.follow_gestures(ui, editor_rect);
        // A capture script moves the view like a camera, from where
        // a `fit` lands once it has
        if let Some(capture) = self.capture.as_mut().filter(|_| self.glide.is_none()) {
            let zoom = capture.take_zoom();
            if zoom != 1.0 {
                self.graph_state.zoom(ui, zoom);
            }
            if let Some(pan) = capture.take_pan(self.graph_state.pan_zoom.pan) {
                self.graph_state.pan_zoom.pan = pan;
            }
        }
        self.user_state.miniatures.clear();
        let graph_start = next_shape();
        // A dive draws all of what will be on screen once it's moved
        let view_clip = ui.clip_rect();
        if dive.is_some() {
            ui.set_clip_rect(canvas);
        }
        // A cable dropped on a full input replaces the one there; keep it in case it's refused
        asks.before_drop = crate::graph::refused_cable::before_drop(
            &self.graph_state.graph,
            self.graph_state.connection_in_progress.is_some(),
        );
        let graph_response = self.graph_state.draw_graph_editor(
            ui,
            AllNodeTemplates,
            &mut self.user_state,
            Vec::default(),
        );
        ui.set_clip_rect(view_clip);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.user_state.cable_spotlight.end_frame(graph_response.hovered_cable, dt);
        self.tend_probe(ctx, graph_response.hovered_cable);
        self.note_miniatures();
        if let Some(dive) = &dive {
            self.paint_dive(ui, layer, &[(grid_start, grid_end), (graph_start, next_shape())], dive);
        }
        // Zooming moves every node; undo keeps positions that don't
        self.history.follow_zoom(zoom_before, pan_before, &self.graph_state.pan_zoom);
        self.follow_touch_pan(ctx);
        self.remember_node_sizes(ctx);
        self.reveal_new_sampler(ctx);
        self.user_state.view_origin = self.history.view_origin();

        // A selection box takes in the frames and notes wholly inside it
        if let Some(start) = self.graph_state.ongoing_box_selection {
            if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                self.user_state.annotations.select_within(egui::Rect::from_two_pos(start, pointer));
            }
        }

        asks.cursor_in_editor = graph_response.cursor_in_editor;

        // Disable the built-in node finder - we use our own context menu
        self.graph_state.node_finder = None;

        // Process graph responses
        for response in graph_response.node_responses {
            self.take_node_response(response, asks);
        }
    }

    /// Acts on what the graph editor said happened, or notes it in `asks`
    /// for once the graph is drawn.
    fn take_node_response(&mut self, response: NodeResponse<crate::graph::SynthResponse, SynthNodeData>, asks: &mut CanvasAsks) {
        match response {
            NodeResponse::CreatedNode(node_id) => {
                // Allocate engine node ID for the new node
                let engine_node_id = self.user_state.allocate_engine_node_id(node_id);
                asks.commands.extend(engine_sync::add_module(
                    &self.graph_state.graph,
                    node_id,
                    engine_node_id,
                ));
            }
            NodeResponse::DeleteNodeFull { node_id, node } => {
                // Get engine node ID before removing from mapping
                if let Some(engine_node_id) = self.user_state.remove_node(node_id) {
                    asks.commands.push(EngineCommand::RemoveModule {
                        node_id: engine_node_id,
                    });
                }
                // A group goes with everything in it
                if let Some(id) = node.user_data.kind.group() {
                    asks.deleted_groups.push(id);
                }
            }
            NodeResponse::ConnectEventEnded { output, input, .. } => {
                // Validate the connection after it was made
                if let Some(error_msg) = self.validate_and_check_connection(output, input) {
                    // Mark for removal
                    asks.invalid_connections.push((output, input));
                    // Show error message
                    self.user_state.set_validation_error(error_msg);
                }
                // The engine hears about it with the frame's other cables
            }
            NodeResponse::User(crate::graph::SynthResponse::ToggleBypass(node_id)) => {
                asks.bypass_toggles.push(node_id);
            }
            NodeResponse::User(response) => match node_menu_action(response) {
                Ok(action) => asks.node_menu_actions.push(action),
                Err(response) => self.take_user_response(response),
            },
            _ => {
                // Other responses not yet handled
            }
        }
    }

    /// Acts on what a module's face asked for: its knobs, pads, samples and MIDI Learn.
    fn take_user_response(&mut self, response: crate::graph::SynthResponse) {
        match response {
            crate::graph::SynthResponse::ParameterChanged {
                node_id: response_node_id,
                param_name,
                value,
            } => {
                // Handle parameter changes from bottom_ui knobs
                // Find the input param by name and update its value
                if let Some(node) = self.graph_state.graph.nodes.get_mut(response_node_id) {
                    if let Some((_name, input_id)) = node.inputs.iter().find(|(name, _)| *name == param_name) {
                        let input_id = *input_id;
                        if let Some(input) = self.graph_state.graph.inputs.get_mut(input_id) {
                            input.value.set_actual_value(value);
                        }
                    }
                }
            }
            crate::graph::SynthResponse::PlayParameter { node_id: response_node_id, param_name, value } => {
                self.play_parameter(response_node_id, &param_name, value);
            }
            crate::graph::SynthResponse::Audition { node_id: response_node_id, chord } => {
                self.audition(response_node_id, chord);
            }
            crate::graph::SynthResponse::CueSample { node_id: response_node_id, from, to } => {
                self.cue_sample(response_node_id, from, to);
            }
            crate::graph::SynthResponse::StopCue(_) => {
                self.stop_cue();
            }
            crate::graph::SynthResponse::TidySample { node_id: response_node_id, tidy } => {
                self.tidy_sample(response_node_id, tidy);
            }
            crate::graph::SynthResponse::DownloadSample(response_node_id) => {
                self.download_sample(response_node_id);
            }
            crate::graph::SynthResponse::Scene { node_id: response_node_id, slot, action } => {
                self.scene_pad(response_node_id, slot, action);
            }
            crate::graph::SynthResponse::SetSliceMarkers { node_id: response_node_id, label, markers } => {
                self.set_slice_markers(response_node_id, label, markers);
            }
            crate::graph::SynthResponse::EditParameters {
                node_id: response_node_id,
                label,
                changes,
            } => {
                if let Some(node) = self.graph_state.graph.nodes.get(response_node_id) {
                    let inputs: Vec<_> = changes
                        .iter()
                        .filter_map(|(name, value)| {
                            node.inputs.iter().find(|(input, _)| input == name).map(|(_, id)| (*id, *value))
                        })
                        .collect();
                    for (input_id, value) in inputs {
                        if let Some(input) = self.graph_state.graph.inputs.get_mut(input_id) {
                            input.value.set_actual_value(value);
                        }
                    }
                }
                self.history.name_next(label);
            }
            crate::graph::SynthResponse::SetLabels { node_id: response_node_id, label, labels } => {
                if let Some(node) = self.graph_state.graph.nodes.get_mut(response_node_id) {
                    for (key, text) in labels {
                        if text.is_empty() {
                            node.user_data.labels.remove(&key);
                        } else {
                            node.user_data.labels.insert(key, text);
                        }
                    }
                }
                self.history.name_next(label);
            }
            crate::graph::SynthResponse::MidiLearnStart {
                engine_node_id,
                param_index,
                param_name,
                min_value,
                max_value,
                control,
                scene,
                into,
            } => {
                // Start MIDI Learn mode for this control
                self.start_midi_learn(MidiLearnTarget {
                    node_id: engine_node_id,
                    param_index,
                    param_name,
                    min_value,
                    max_value,
                    control,
                    scene,
                    into: into.map(|into| *into),
                });
            }
            crate::graph::SynthResponse::MidiStyle { engine_node_id, param_index, scene, style } => {
                self.restyle_mapping(engine_node_id, param_index, scene, style);
            }
            crate::graph::SynthResponse::MidiLearnCancel => {
                self.cancel_midi_learn();
            }
            crate::graph::SynthResponse::MapToMacro {
                engine_node_id,
                param_index,
                param_name,
                min_value,
                max_value,
                slot,
            } => {
                self.map_to_macro(MidiMapping::from_macro(slot, engine_node_id, param_index, param_name, min_value, max_value));
            }
            crate::graph::SynthResponse::MidiLearnClear {
                engine_node_id,
                param_index,
                scene,
            } => {
                self.clear_mapping_for_param(engine_node_id, param_index, scene);
            }
            _ => {
                // Other responses not yet handled
            }
        }
    }

    /// Opens the add menu on a right-click or long press on empty canvas,
    /// then draws it.
    fn open_add_menu(&mut self, ctx: &egui::Context, cursor_in_editor: bool) {
        // Detect right-click in editor to open context menu
        let mut menu_just_opened = false;
        let right_click = ctx.input(|i| i.pointer.secondary_clicked().then(|| i.pointer.interact_pos()).flatten());
        // A finger's right-click: held still on the canvas
        let long_press = self.long_press.update(ctx);
        // Only show "add node" menu when clicking on empty canvas, not on nodes/widgets
        if (right_click.is_some() || long_press.is_some()) && cursor_in_editor {
            // Only open if not already showing a menu and no widget context menu is open
            if self.user_state.context_menu_pos.is_none()
                && !self.user_state.widget_context_menu_open
                && !self.user_state.over_annotation
            {
                let click_pos = right_click.or(long_press);
                // A module has its own menu, and its knobs theirs. Neither says
                // so on the frame of the click, so go by where the click was
                if let Some(click_pos) = click_pos.filter(|pos| !self.is_over_module(ctx, *pos)) {
                    self.user_state.context_menu_pos = Some(click_pos);
                    self.add_menu = AddMenu::opened(right_click.is_none());
                    self.my_modules = library::list();
                    menu_just_opened = true;
                }
            }
        }

        // Show custom context menu for adding nodes
        self.show_add_menu(ctx, menu_just_opened);
    }

    /// The quick-add palette
    fn show_quick_add(&mut self, ctx: &egui::Context) {
        if let Some(palette) = &mut self.quick_add {
            match palette.show(ctx) {
                PaletteAction::None => {}
                PaletteAction::Close => self.quick_add = None,
                PaletteAction::Add(template) => {
                    let anchor = palette.anchor();
                    self.quick_add = None;
                    self.add_module_at(template, anchor);
                }
                PaletteAction::AddSaved(saved) => {
                    let anchor = palette.anchor();
                    self.quick_add = None;
                    self.add_saved_module(&saved, anchor);
                }
            }
        }
    }

    fn handle_node_menu(&mut self, ctx: &egui::Context, action: NodeMenuAction) {
        match action {
            NodeMenuAction::Select(node_id) => {
                if !self.graph_state.selected_nodes.contains(&node_id) {
                    self.graph_state.selected_nodes = vec![node_id];
                }
            }
            NodeMenuAction::Duplicate(node_id) => self.duplicate_selection(&self.menu_targets(node_id)),
            NodeMenuAction::Copy(node_id) => self.copy_selection(ctx, &self.menu_targets(node_id)),
            NodeMenuAction::Reset(node_id) => self.reset_modules(&self.menu_targets(node_id).nodes),
            NodeMenuAction::Delete(node_id) => self.delete_selection(&self.menu_targets(node_id), "Delete", "Deleted"),
            NodeMenuAction::Group(node_id) => self.group_selection(ctx, &self.menu_targets(node_id).nodes),
            NodeMenuAction::Ungroup(node_id) => self.ungroup(ctx, &self.menu_targets(node_id).nodes),
            NodeMenuAction::Enter(node_id) => self.dive_into(ctx, node_id),
            NodeMenuAction::StartRename(node_id) => self.start_rename(node_id),
            NodeMenuAction::Rename(node_id, name) => self.rename_group(node_id, &name),
            NodeMenuAction::Pin(node_id, param_name, levels) => self.pin_knob(node_id, &param_name, levels),
            NodeMenuAction::NameFace(node_id, param_name, name) => self.name_face_control(node_id, &param_name, &name),
            NodeMenuAction::Jack(node_id, side, port, edit) => self.edit_jack(node_id, side, &port, edit),
            NodeMenuAction::SaveToLibrary(node_id) => self.save_to_library(node_id),
            NodeMenuAction::OpenSample(node_id) => self.open_sample_dialog(node_id),
            NodeMenuAction::RecordInto(node_id, source, bars) => self.record_into_new_sampler(node_id, &source, bars),
        }
    }
}

/// The module menu item a response stands for, or the response back if
/// it's something else.
fn node_menu_action(response: crate::graph::SynthResponse) -> Result<NodeMenuAction, crate::graph::SynthResponse> {
    use crate::graph::SynthResponse;
    Ok(match response {
        SynthResponse::NodeSelected(node_id) => NodeMenuAction::Select(node_id),
        SynthResponse::DuplicateNode(node_id) => NodeMenuAction::Duplicate(node_id),
        SynthResponse::CopyNode(node_id) => NodeMenuAction::Copy(node_id),
        SynthResponse::ResetNode(node_id) => NodeMenuAction::Reset(node_id),
        SynthResponse::DeleteNode(node_id) => NodeMenuAction::Delete(node_id),
        SynthResponse::GroupNode(node_id) => NodeMenuAction::Group(node_id),
        SynthResponse::UngroupNode(node_id) => NodeMenuAction::Ungroup(node_id),
        SynthResponse::EnterGroup(node_id) => NodeMenuAction::Enter(node_id),
        SynthResponse::StartRename(node_id) => NodeMenuAction::StartRename(node_id),
        SynthResponse::RenameGroup { node_id, name } => NodeMenuAction::Rename(node_id, name),
        SynthResponse::PinKnob { node_id, param_name, levels } => NodeMenuAction::Pin(node_id, param_name, levels),
        SynthResponse::NameFaceControl { node_id, param_name, name } => NodeMenuAction::NameFace(node_id, param_name, name),
        SynthResponse::Jack { node_id, side, port, edit } => NodeMenuAction::Jack(node_id, side, port, edit),
        SynthResponse::SaveGroup(node_id) => NodeMenuAction::SaveToLibrary(node_id),
        SynthResponse::OpenSample(node_id) => NodeMenuAction::OpenSample(node_id),
        SynthResponse::RecordInto { node_id, source, bars } => NodeMenuAction::RecordInto(node_id, source, bars),
        other => return Err(other),
    })
}
