//! How a group's nodes look: the group's own node, with its name, a
//! miniature of what's inside it and the controls pinned to it, and the Inputs
//! and Outputs nodes inside it.
//!
//! A group wears rose, the one header colour no built-in module has, so a
//! module of your own making reads as yours at a glance. Its face shows the
//! modules inside as small cards in their own colours, wired as they are,
//! and opens the group when clicked.

use egui::epaint::CubicBezierShape;
use egui::text::{CCursor, CCursorRange};
use egui::emath::TSTransform;
use egui::{vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke};
use egui_node_graph2::{NodeId, NodeResponse};

use crate::app::theme;
use super::groups::{Control, FaceControl, JackAction, JackEdit, NodeKind, Preview, Side};
use super::hints;
use super::steady_text;
use super::node_data::KnobPlace;
use super::{SynthGraph, SynthGraphState, SynthNodeData, SynthResponse, SynthValueType};

type Responses = Vec<NodeResponse<SynthResponse, SynthNodeData>>;

/// Width of the miniature on a group's face, unzoomed.
const PREVIEW_WIDTH: f32 = 176.0;
/// Its height follows the shape of what's inside, within these.
const PREVIEW_MIN_HEIGHT: f32 = 46.0;
const PREVIEW_MAX_HEIGHT: f32 = 100.0;
/// Pinned knobs per row on a group's face.
const KNOBS_PER_ROW: usize = 4;

/// The miniature's backing, a little deeper than the node body.
const PREVIEW_FILL: Color32 = Color32::from_rgb(28, 30, 40);
/// A miniature module's body.
const CARD_FILL: Color32 = Color32::from_rgb(58, 61, 74);

/// The header colour of a group's node, or of its Inputs and Outputs,
/// which wear it more quietly.
pub fn header_color(kind: NodeKind) -> Color32 {
    match kind {
        NodeKind::Group(_) => theme::module::GROUP,
        _ => theme::module::GROUP.lerp_to_gamma(Color32::from_gray(96), 0.45),
    }
}

/// The header of a group's node or its Inputs or Outputs: its icon and its
/// right-click menu. A group opens on a double-click.
pub fn top_bar(data: &SynthNodeData, ui: &mut egui::Ui, node_id: NodeId, user_state: &mut SynthGraphState, zoom: f32) -> Responses {
    let mut responses = Vec::new();
    if let Some(window) = ui.ctx().read_response(egui::Id::new((node_id, "window"))) {
        if data.kind.group().is_some() {
            if window.double_clicked() {
                responses.push(NodeResponse::User(SynthResponse::EnterGroup(node_id)));
            }
            if window.secondary_clicked() {
                responses.push(NodeResponse::User(SynthResponse::NodeSelected(node_id)));
            }
            if window.context_menu(|ui| group_menu(ui, node_id, &mut responses)).is_some() {
                user_state.widget_context_menu_open = true;
            }
        }
    }

    let size = 14.0 * zoom;
    let (rect, response) = ui.allocate_exact_size(vec2(size + 4.0 * zoom, size), Sense::hover());
    if !data.description.is_empty() {
        response.on_hover_text(data.description);
    }
    let center = Pos2::new(rect.left() + size * 0.5, rect.center().y);
    let ink = data.titlebar_ink();
    match data.kind {
        NodeKind::Group(_) => draw_stack(ui.painter(), center, size, ink),
        NodeKind::Inputs(_) => draw_doorway(ui.painter(), center, size, ink, true),
        NodeKind::Outputs(_) => draw_doorway(ui.painter(), center, size, ink, false),
        NodeKind::Module => {}
    }
    responses
}

/// A group's right-click menu.
fn group_menu(ui: &mut egui::Ui, node_id: NodeId, responses: &mut Responses) {
    ui.set_min_width(190.0);
    let mut item = |ui: &mut egui::Ui, label: &str, shortcut: &str, response: SynthResponse| {
        if ui.add(egui::Button::new(label).shortcut_text(crate::app::shortcuts::keys(ui.ctx(), shortcut))).clicked() {
            responses.push(NodeResponse::User(response));
            ui.close_menu();
        }
    };
    item(ui, "Open", "Tab", SynthResponse::EnterGroup(node_id));
    item(ui, "Rename", "F2", SynthResponse::StartRename(node_id));
    item(ui, "Save to My Modules", "", SynthResponse::SaveGroup(node_id));
    ui.separator();
    item(ui, "Duplicate", "Ctrl+D", SynthResponse::DuplicateNode(node_id));
    item(ui, "Copy", "Ctrl+C", SynthResponse::CopyNode(node_id));
    item(ui, "Ungroup", "Ctrl+Alt+G", SynthResponse::UngroupNode(node_id));
    ui.separator();
    item(ui, "Delete", "Del", SynthResponse::DeleteNode(node_id));
}

/// A port's right-click menu, when it has anything to do with groups'
/// jacks: on a module inside a group, on a group's node, or on its Inputs
/// or Outputs. `response` is the port's label.
pub fn port_menu(
    response: &egui::Response,
    node_id: NodeId,
    side: Side,
    port: &str,
    user_state: &mut SynthGraphState,
) -> Option<SynthResponse> {
    if user_state.jack_actions(node_id, side, port).is_empty() {
        return None;
    }
    port_menu_with(response, node_id, side, port, user_state, None)
}

/// A port's right-click menu: what it can do with groups' jacks, if
/// anything, and then whatever `more` adds below them.
pub fn port_menu_with(
    response: &egui::Response,
    node_id: NodeId,
    side: Side,
    port: &str,
    user_state: &mut SynthGraphState,
    more: Option<&dyn Fn(&mut egui::Ui) -> Option<SynthResponse>>,
) -> Option<SynthResponse> {
    let actions = user_state.jack_actions(node_id, side, port).to_vec();
    // Each time the menu opens, its name field starts from the jack's name
    if response.secondary_clicked() {
        reset_name_field(&response.ctx, jack_name_id(node_id, side, port));
    }
    let mut chosen = None;
    let menu = response.context_menu(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        if !actions.is_empty() {
            chosen = jack_items(ui, &actions, node_id, side, port);
        }
        if let Some(more) = more {
            if !actions.is_empty() {
                ui.separator();
            }
            let extra = more(ui);
            chosen = chosen.take().or(extra);
        }
    });
    if menu.is_some() {
        user_state.widget_context_menu_open = true;
    }
    chosen
}

/// Where a jack's menu keeps the name being typed.
fn jack_name_id(node_id: NodeId, side: Side, port: &str) -> egui::Id {
    egui::Id::new((node_id, side, port, "jack name"))
}

/// Where a face control's menu keeps the name being typed.
pub fn face_name_id(node_id: NodeId, param_name: &str) -> egui::Id {
    egui::Id::new((node_id, param_name, "face name"))
}

/// Starts a menu's name field afresh, from the name it names, as the menu
/// opens. Not while it's open: the menu's code runs more than once a frame,
/// and clearing it then would swallow the typing.
pub fn reset_name_field(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.remove::<(String, bool)>(id));
}

/// The menu items for what a port can do with groups' jacks. A jack's own
/// menu opens on a field with its name selected, so typing renames it.
pub fn jack_items(ui: &mut egui::Ui, actions: &[JackAction], node_id: NodeId, side: Side, port: &str) -> Option<SynthResponse> {
    let mut chosen = None;
    let choose = |edit: JackEdit| Some(SynthResponse::Jack { node_id, side, port: port.to_string(), edit });
    for (i, &action) in actions.iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        let item = |ui: &mut egui::Ui, enabled: bool, label: &str, hint: &str, edit: JackEdit| {
            let button = ui.add_enabled(enabled, egui::Button::new(label));
            let button = if enabled { button.on_hover_text(hint) } else { button.on_disabled_hover_text(hint) };
            if button.clicked() {
                ui.close_menu();
                return choose(edit);
            }
            None
        };
        let picked = match action {
            JackAction::Show => item(ui, true, "Show on group", "Give the group a jack for this port", JackEdit::Show),
            JackAction::Taken => {
                item(ui, false, "Show on group", "Something inside the group is plugged in here already", JackEdit::Show)
            }
            JackAction::Hide => item(
                ui,
                true,
                "Hide from group",
                "Unplug it from the group's jack. A jack left with nothing inside goes",
                JackEdit::Hide,
            ),
            JackAction::Jack { slot, count } => {
                let renamed = name_field(ui, jack_name_id(node_id, side, port), port, "Type a new name and press Enter")
                    .filter(|name| !name.trim().is_empty())
                    .map(JackEdit::Rename)
                    .and_then(&choose);
                ui.separator();
                let up = item(ui, slot > 0, "Move up", "One place up the group's jacks", JackEdit::Move { up: true });
                let down = item(ui, slot + 1 < count, "Move down", "One place down the group's jacks", JackEdit::Move { up: false });
                ui.separator();
                let remove = item(ui, true, "Remove jack", "Take this jack off the group, with its cables inside and out", JackEdit::Remove);
                renamed.or(up).or(down).or(remove)
            }
        };
        chosen = chosen.or(picked);
    }
    chosen
}

/// A name in a menu (a jack's, a face control's), ready to type over, kept
/// at `id` while the menu's open. Enter renames it: returns what was typed,
/// if it's new, blank included.
pub fn name_field(ui: &mut egui::Ui, id: egui::Id, current: &str, hint: &str) -> Option<String> {
    let (mut text, fresh) = ui.data_mut(|d| d.get_temp::<(String, bool)>(id)).unwrap_or_else(|| (current.to_string(), true));
    let mut renamed = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").small().color(theme::text::SECONDARY));
        let edit_id = id.with("edit");
        let mut output = egui::TextEdit::singleline(&mut text).id(edit_id).desired_width(120.0).show(ui);
        if fresh {
            output.response.request_focus();
            let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
            output.state.cursor.set_char_range(Some(all));
            output.state.store(ui.ctx(), edit_id);
        }
        let response = output.response.on_hover_text(hint);
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if text.trim() != current {
                renamed = Some(text.clone());
            }
            ui.close_menu();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, (text, false)));
    renamed
}

/// Two cards, one behind the other: a group of modules.
fn draw_stack(painter: &egui::Painter, center: Pos2, size: f32, ink: Color32) {
    let side = size * 0.62;
    let back = Rect::from_center_size(center + vec2(size * 0.14, -size * 0.14), vec2(side, side));
    let front = Rect::from_center_size(center + vec2(-size * 0.12, size * 0.12), vec2(side, side));
    let rounding = size * 0.12;
    painter.rect_stroke(back, rounding, Stroke::new((size * 0.1).max(1.0), ink.gamma_multiply(0.75)));
    painter.rect_filled(front.expand(size * 0.07), rounding, theme::module::GROUP);
    painter.rect_filled(front, rounding, ink);
}

/// An arrow through an edge: signals coming in through the group's input
/// jacks, or going out through its outputs.
fn draw_doorway(painter: &egui::Painter, center: Pos2, size: f32, ink: Color32, inward: bool) {
    let s = size * 0.5;
    let stroke = Stroke::new((size * 0.12).max(1.0), ink);
    let edge_x = if inward { center.x - s * 0.55 } else { center.x + s * 0.55 };
    painter.line_segment([Pos2::new(edge_x, center.y - s * 0.8), Pos2::new(edge_x, center.y + s * 0.8)], stroke);
    let (from, to) = (center.x - s * 0.9, center.x + s * 0.9);
    painter.line_segment([Pos2::new(from, center.y), Pos2::new(to, center.y)], stroke);
    painter.line_segment([Pos2::new(to, center.y), Pos2::new(to - s * 0.45, center.y - s * 0.45)], stroke);
    painter.line_segment([Pos2::new(to, center.y), Pos2::new(to - s * 0.45, center.y + s * 0.45)], stroke);
}

/// The body of a group's node: the name being typed, if it is, then the
/// miniature, then the pinned dropdowns and toggles, then the pinned knobs.
pub fn body(
    data: &SynthNodeData,
    ui: &mut egui::Ui,
    node_id: NodeId,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    zoom: f32,
) -> Responses {
    let mut responses = Vec::new();
    let Some(id) = data.kind.group() else {
        // Inputs or Outputs, which say so when there's nothing to show
        let node = &graph[node_id];
        let none = match data.kind {
            NodeKind::Inputs(_) if node.outputs.is_empty() => "No input jacks",
            NodeKind::Outputs(_) if node.inputs.is_empty() => "No output jacks",
            _ => return responses,
        };
        ui.label(RichText::new(none).small().color(theme::text::QUIET));
        return responses;
    };
    ui.add_space(4.0 * zoom);
    rename_field(ui, node_id, data, user_state, zoom, &mut responses);

    let face = user_state.group_faces.get(&id).cloned().unwrap_or_default();
    let preview = &face.preview;
    let width = PREVIEW_WIDTH * zoom;
    let aspect = if preview.bounds.is_positive() { preview.bounds.width() / preview.bounds.height() } else { 3.0 };
    let height = (width / aspect).clamp(PREVIEW_MIN_HEIGHT * zoom, PREVIEW_MAX_HEIGHT * zoom);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter_at(rect.expand(1.0));
    paint_miniature(&painter, rect, preview, zoom, hovered);
    if hovered {
        paint_open_pill(&painter, rect, zoom);
    }
    user_state.miniatures.insert(id, rect);
    if response.on_hover_text("Open the group (double-click it, or Tab)").clicked() {
        responses.push(NodeResponse::User(SynthResponse::EnterGroup(node_id)));
    }

    // The dropdowns and toggles pinned to it, then the knobs, in their
    // modules' colours
    let outer = data.parent.is_some();
    let (knobs, switches): (Vec<_>, Vec<_>) = face.controls.iter().partition(|c| c.kind == Control::Knob);
    if !switches.is_empty() {
        ui.add_space(6.0 * zoom);
        for (row, width) in switch_rows(ui, graph, &switches, PREVIEW_WIDTH * zoom) {
            ui.horizontal(|ui| {
                for control in row {
                    switch_chip(ui, control, graph, user_state, width, outer, &mut responses);
                }
            });
        }
    }
    if !knobs.is_empty() {
        ui.add_space(6.0 * zoom);
        for row in knobs.chunks(KNOBS_PER_ROW) {
            ui.horizontal(|ui| {
                for knob in row {
                    let Some(module) = graph.nodes.get(knob.node) else { continue };
                    let Some(param) = module.user_data.knob_params.iter().find(|k| k.param_name == knob.param) else {
                        continue;
                    };
                    let place = KnobPlace::Face { depth: knob.depth, outer };
                    module.user_data.knob_cell(ui, knob.node, param, graph, user_state, zoom, &mut responses, place);
                }
            });
        }
    }
    responses
}

/// Where a miniature `rect` big, on a face at `zoom`, draws each point of
/// its preview: what's inside fitted to it, a little in from its edge.
pub fn miniature_transform(rect: Rect, preview: &Preview, zoom: f32) -> Option<TSTransform> {
    let bounds = preview.bounds;
    if preview.boxes.is_empty() || !bounds.is_positive() {
        return None;
    }
    let inner = rect.shrink2(vec2(8.0, 7.0) * zoom);
    let scale = (inner.width() / bounds.width()).min(inner.height() / bounds.height());
    Some(TSTransform::new(inner.center().to_vec2() - bounds.center().to_vec2() * scale, scale))
}

/// The miniature on a group's face: each module inside as a small card in
/// its header colour, wired as it is. `lit` while the pointer's over it.
pub fn paint_miniature(painter: &egui::Painter, rect: Rect, preview: &Preview, zoom: f32, lit: bool) {
    let rounding = 6.0 * zoom;
    let rose = theme::module::GROUP;
    painter.rect(rect, rounding, PREVIEW_FILL, Stroke::new(1.0, rose.gamma_multiply(if lit { 0.8 } else { 0.3 })));

    let Some(to_face) = miniature_transform(rect, preview, zoom) else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Empty", FontId::proportional(11.0 * zoom), theme::text::DISABLED);
        return;
    };
    let map = |p: Pos2| to_face * p;
    let line = (1.3 * zoom).max(0.8);
    for ([a, b], color) in &preview.wires {
        let (a, b) = (map(*a), map(*b));
        let reach = ((b.x - a.x).abs() * 0.5).max(10.0 * zoom);
        painter.add(CubicBezierShape::from_points_stroke(
            [a, a + vec2(reach, 0.0), b - vec2(reach, 0.0), b],
            false,
            Color32::TRANSPARENT,
            Stroke::new(line, color.gamma_multiply(0.8)),
        ));
    }
    for (card, color) in &preview.boxes {
        let card = Rect::from_min_max(map(card.min), map(card.max)).shrink(1.0 * zoom);
        let corner = (2.5 * zoom).min(card.height() * 0.3);
        painter.rect_filled(card, corner, CARD_FILL);
        let band = Rect::from_min_max(card.min, Pos2::new(card.max.x, card.min.y + (card.height() * 0.3).max(2.0)));
        painter.rect_filled(band, corner, *color);
    }
}

/// "Open ▸", over a miniature the pointer is on.
pub fn paint_open_pill(painter: &egui::Painter, rect: Rect, zoom: f32) {
    let rose = theme::module::GROUP;
    let label = "Open  ▸";
    let font = FontId::proportional(12.0 * zoom);
    let galley = painter.layout_no_wrap(label.to_string(), font, theme::text::PRIMARY);
    let pill = Rect::from_center_size(rect.center(), galley.size() + vec2(18.0, 8.0) * zoom);
    painter.rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha(190));
    painter.rect_stroke(pill, pill.height() / 2.0, Stroke::new(1.0, rose.gamma_multiply(0.9)));
    painter.galley(pill.center() - galley.size() / 2.0, galley, theme::text::PRIMARY);
}

/// The pinned dropdowns and toggles laid out in rows across the face, in
/// order, with how wide each of a row's chips is. A row takes as many as fit
/// at half the face or wider, as wide as their longest choice, and they
/// share the row's width evenly, so every row lines up with the miniature.
fn switch_rows<'a>(
    ui: &egui::Ui,
    graph: &SynthGraph,
    switches: &[&'a FaceControl],
    face_width: f32,
) -> Vec<(Vec<&'a FaceControl>, f32)> {
    let gap = ui.spacing().item_spacing.x;
    let wants = |control: &FaceControl| {
        let options = graph.nodes.get(control.node).and_then(|module| {
            let (_, input) = module.inputs.iter().find(|(name, _)| *name == control.param)?;
            match &graph.get_input(*input).value {
                SynthValueType::Select { options, .. } => Some(options.clone()),
                _ => None,
            }
        });
        chip_width(ui, &options.unwrap_or_default(), face_width)
    };

    let mut rows: Vec<(Vec<&FaceControl>, f32)> = Vec::new();
    for &control in switches {
        let width = wants(control);
        match rows.last_mut() {
            Some((row, used)) if *used + gap + width <= face_width + 0.5 => {
                row.push(control);
                *used += gap + width;
            }
            _ => rows.push((vec![control], width)),
        }
    }
    rows.into_iter()
        .map(|(row, _)| {
            let n = row.len() as f32;
            let width = (face_width - gap * (n - 1.0)) / n;
            (row, width)
        })
        .collect()
}

/// How wide a chip offering `options` wants to be on a face `face_width`
/// wide: as wide as its longest choice, at least half the face. Measured as
/// the text would be if fonts scaled smoothly, so the chips sharing a row,
/// and with them the face's height, don't change as the view zooms (#326).
fn chip_width(ui: &egui::Ui, options: &[String], face_width: f32) -> f32 {
    let least = (face_width - ui.spacing().item_spacing.x) / 2.0;
    let longest = options.iter().map(|option| steady_text::dropdown_width(ui, option, 0.0));
    longest.fold(least, f32::max).min(face_width)
}

/// A dropdown or toggle pinned to a group's face, `width` wide: its name in
/// its module's colour over the control, which changes it inside as well.
fn switch_chip(
    ui: &mut egui::Ui,
    control: &FaceControl,
    graph: &SynthGraph,
    user_state: &mut SynthGraphState,
    width: f32,
    outer: bool,
    responses: &mut Responses,
) {
    let Some(module) = graph.nodes.get(control.node) else { return };
    let Some(&(_, input_id)) = module.inputs.iter().find(|(name, _)| *name == control.param) else { return };
    let input = graph.get_input(input_id);
    let data = &module.user_data;
    let accent = data.category.color();
    // A dropdown with a jack is the cable's while one is plugged in
    let patched = graph.iter_connections().any(|(to, _)| to == input_id);
    let place = KnobPlace::Face { depth: control.depth, outer };
    let hint = || data.face_hint(hints::Hint::input(data.module_id, &control.param), &control.param, place);

    ui.vertical(|ui| {
        ui.set_width(width);
        let label = match (data.face_label(&control.param), &input.value) {
            (Some(name), _) => name,
            (None, SynthValueType::Select { label, .. } | SynthValueType::Toggle { label, .. }) if !label.is_empty() => label.as_str(),
            _ => control.param.as_str(),
        };
        let name = steady_text::truncated_label(
            ui,
            RichText::new(label).small().color(accent.gamma_multiply(if patched { 0.5 } else { 0.9 })),
            Sense::click(),
        );
        let name = hints::attach(name, hint());

        let mut value = input.value.clone();
        let response = ui
            .scope(|ui| {
                if patched {
                    ui.disable();
                }
                let visuals = &mut ui.visuals_mut().widgets;
                visuals.inactive.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.45));
                visuals.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.8));
                match &mut value {
                    SynthValueType::Select { value, options, .. } => {
                        steady_text::steady_dropdowns(ui);
                        egui::ComboBox::from_id_salt((control.node, control.param.as_str(), "face"))
                            .width(width)
                            // A long choice ("Pentatonic Minor") is cut short to
                            // fit the face, and shown whole in the list
                            .truncate()
                            .selected_text(options.get(*value).map(|s| s.as_str()).unwrap_or(""))
                            .show_ui(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                for (i, option) in options.iter().enumerate() {
                                    ui.selectable_value(value, i, option);
                                }
                            })
                            .response
                    }
                    SynthValueType::Toggle { value, .. } => {
                        // Lit in the module's colour when on
                        let selection = &mut ui.visuals_mut().selection;
                        selection.bg_fill = accent.gamma_multiply(0.4);
                        selection.stroke = Stroke::new(1.0, accent.lerp_to_gamma(Color32::WHITE, 0.55));
                        steady_text::steady_buttons(ui);
                        let text = if *value { "On" } else { "Off" };
                        let button = ui.add(egui::Button::new(text).selected(*value).min_size(vec2(width, 0.0)));
                        if button.clicked() {
                            *value = !*value;
                        }
                        button
                    }
                    SynthValueType::Port | SynthValueType::Number { .. } => steady_text::label(ui, "", Sense::hover()),
                }
            })
            .inner;
        let response = hints::attach(response, hint());
        if value != input.value {
            responses.push(NodeResponse::User(SynthResponse::ParameterChanged {
                node_id: control.node,
                param_name: control.param.clone(),
                value: value.actual_value(),
            }));
        }

        for response in [name, response] {
            if let Some(pin) = data.switch_menu(&response, control.node, &control.param, label, place, user_state) {
                responses.push(NodeResponse::User(pin));
            }
        }
    });
}

/// The field a group's name is typed into, while it's being named.
fn rename_field(
    ui: &mut egui::Ui,
    node_id: NodeId,
    data: &SynthNodeData,
    user_state: &mut SynthGraphState,
    zoom: f32,
    responses: &mut Responses,
) {
    let Some(id) = data.kind.group() else { return };
    let Some(renaming) = user_state.renaming.as_mut().filter(|r| r.group == id) else { return };
    let edit_id = egui::Id::new((node_id, "group name"));
    let mut output = egui::TextEdit::singleline(&mut renaming.text)
        .id(edit_id)
        .font(FontId::proportional(14.0 * zoom))
        .hint_text("Name this group")
        .desired_width(PREVIEW_WIDTH * zoom)
        .show(ui);
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if renaming.fresh {
        renaming.fresh = false;
        output.response.request_focus();
        let all = CCursorRange::two(CCursor::new(0), CCursor::new(renaming.text.chars().count()));
        output.state.cursor.set_char_range(Some(all));
        output.state.store(ui.ctx(), edit_id);
    } else if output.response.lost_focus() || !output.response.has_focus() {
        let name = renaming.text.trim().to_string();
        user_state.renaming = None;
        if !escape && !name.is_empty() {
            responses.push(NodeResponse::User(SynthResponse::RenameGroup { node_id, name }));
        }
    }
    ui.label(RichText::new("Enter to name it, Esc to keep the name").small().color(theme::text::QUIET));
    ui.add_space(4.0 * zoom);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chip wants the same share of the face at every zoom, so the chips
    /// sharing a row don't change as the view zooms (#326).
    #[test]
    fn a_chip_keeps_its_share_of_the_face_as_the_view_zooms() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let options: Vec<String> = ["Major", "Pentatonic Minor", "Dorian"].map(String::from).into();
        let mut seen = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput { viewport_id: egui::ViewportId::ROOT, ..Default::default() };
            ctx.set_pixels_per_point(1.25);
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    seen.clear();
                    let style = ui.style().clone();
                    for step in 0..60 {
                        let zoom = 0.3 * (2.0f32 / 0.3).powf(step as f32 / 59.0);
                        // The canvas scales the style as it zooms
                        let mut zoomed = (*style).clone();
                        for font in zoomed.text_styles.values_mut() {
                            font.size *= zoom;
                        }
                        let spacing = &mut zoomed.spacing;
                        spacing.item_spacing *= zoom;
                        spacing.button_padding *= zoom;
                        spacing.icon_width *= zoom;
                        spacing.icon_spacing *= zoom;
                        ui.set_style(zoomed);
                        seen.push((zoom, chip_width(ui, &options, PREVIEW_WIDTH * zoom) / zoom));
                    }
                    ui.set_style(style);
                });
            });
        }
        let (_, first) = seen[0];
        assert!(first > PREVIEW_WIDTH / 2.0, "the longest choice is wider than half the face: {first}");
        for (zoom, width) in &seen {
            assert!((width - first).abs() < 0.01 * first, "{width} at zoom {zoom}, {first} at the first");
        }
    }
}
