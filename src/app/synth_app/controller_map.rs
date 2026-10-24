//! The Controller Map (#426): every MIDI mapping in the patch on one
//! panel, in the order they were learned. Each row says what on the
//! controller turns what in the patch, and edits how: the range it turns a
//! knob across, typed in the knob's own units, whether the hardware is a
//! fader or an endless encoder, whether a fader picks the knob up, and
//! which way up it goes. A row clears its mapping, and hovering one lights
//! its control on the canvas.
//!
//! Edits go through [`MidiMappings`](crate::app::midi_mappings::MidiMappings),
//! as the knobs' own menus' do, so they're unsaved changes to the patch
//! until it's saved. A host macro is listed, but the DAW has its range.

use egui::text::{CCursor, CCursorRange};

use super::*;
use crate::graph::map_mode::MapKey;
use crate::graph::{MidiStyle, NumberSpec, SynthValueType};
use crate::persistence::{MidiControl, MidiEncoding, MidiMapping, MidiSource};

/// The panel: open or not.
#[derive(Debug, Default)]
pub(super) struct ControllerMap {
    pub open: bool,
}

/// One mapping, as its row shows it.
struct Row {
    key: MapKey,
    mapping: MidiMapping,
    /// What turns it: "CC 74 · ch 1", "C2 · ch 10", "Macro 3".
    source: String,
    module: Option<String>,
    /// The module's colour, for the dot by its name.
    accent: egui::Color32,
    /// The knob's range, units and curve, for a knob its CC turns.
    spec: Option<NumberSpec>,
}

/// What a row asked for.
enum Edit {
    Style(MapKey, MidiStyle),
    Range(MapKey, (f32, f32), (f32, f32)),
    Clear(MapKey),
}

/// The columns' widths, in order: source, module, control, range,
/// hardware, pick up, invert, and clear.
const COLUMNS: [f32; 8] = [96.0, 120.0, 100.0, 196.0, 132.0, 52.0, 44.0, 18.0];
const ROW_HEIGHT: f32 = 24.0;

impl SynthApp {
    /// Opens the Controller Map, or closes it.
    pub(super) fn toggle_controller_map(&mut self) {
        self.controller_map.open = !self.controller_map.open;
    }

    /// The Controller Map, while it's open, and the control its hovered
    /// row is for lit on the canvas.
    pub(super) fn show_controller_map(&mut self, ctx: &egui::Context) {
        if !self.controller_map.open {
            self.user_state.map_mode.highlight = None;
            return;
        }
        let rows: Vec<Row> = self.midi.mappings.iter().map(|mapping| self.controller_row(mapping)).collect();
        let map_mode = self.map_mode_on();
        let mut open = true;
        let mut edits = Vec::new();
        let mut hovered = None;
        let mut toggle_map_mode = false;
        let editor = self.editor_rect;
        let width: f32 = COLUMNS.iter().sum::<f32>() + 6.0 * (COLUMNS.len() - 1) as f32;
        egui::Window::new("Controller Map")
            .id(egui::Id::unique("controller_map"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(editor.right_top() + egui::vec2(-16.0, 16.0))
            .show(ctx, |ui| {
                ui.set_width(width);
                if heading(ui, rows.len()) {
                    open = false;
                }
                ui.add_space(8.0);
                if rows.is_empty() {
                    empty(ui);
                } else {
                    header(ui);
                    let room = (editor.height() - 160.0).max(120.0);
                    egui::ScrollArea::vertical().max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                        for row in &rows {
                            if draw_row(ui, row, &mut edits) {
                                hovered = Some(row.key);
                            }
                        }
                    });
                }
                ui.add_space(8.0);
                toggle_map_mode = footer(ui, map_mode);
            });
        self.controller_map.open &= open;
        self.user_state.map_mode.highlight = hovered.filter(|_| self.controller_map.open);
        if toggle_map_mode {
            self.toggle_map_mode();
        }
        for edit in edits {
            match edit {
                Edit::Style(key, style) => self.restyle_mapping(key.node, key.param_index, key.scene, style),
                Edit::Range(key, range, limits) => {
                    if self.midi.mappings.set_range(key.node, key.param_index, range, limits) {
                        self.show_midi_badges();
                    }
                }
                Edit::Clear(key) => self.clear_mapping_for_param(key.node, key.param_index, key.scene),
            }
        }
    }

    /// A mapping's row: what turns it, and what it turns.
    fn controller_row(&self, mapping: &MidiMapping) -> Row {
        let key = MapKey { node: mapping.node_id, param_index: if mapping.scene.is_some() { 0 } else { mapping.param_index }, scene: mapping.scene };
        let node = self.user_state.graph_node_id(mapping.node_id).and_then(|node| self.graph_state.graph.nodes.get(node).map(|n| (node, n)));
        let accent = node.map_or(theme::text::DISABLED, |(_, n)| n.user_data.category.color());
        let spec = node
            .filter(|_| mapping.scene.is_none() && mapping.macro_slot.is_none() && mapping.control == MidiControl::Knob)
            .and_then(|(node, _)| self.param_input(node, mapping.param_index))
            .and_then(|input| match &self.graph_state.graph.get_input(input).value {
                SynthValueType::Number { spec, .. } => Some(*spec),
                _ => None,
            });
        Row {
            key,
            mapping: mapping.clone(),
            source: crate::app::midi_mappings::info(mapping).text(),
            module: node.map(|(_, n)| n.user_data.display_name.clone()),
            accent,
            spec,
        }
    }
}

/// The panel's name, after a short purple bar, how many mappings it holds,
/// and the way to close it. True when it's closed.
fn heading(ui: &mut egui::Ui, count: usize) -> bool {
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 1.5, theme::signal::MIDI);
        let family = egui::FontFamily::Name(theme::TITLE_FAMILY.into());
        ui.label(RichText::new("Controller Map").font(egui::FontId::new(16.0, family)).color(theme::text::PRIMARY));
        let count = match count {
            0 => "nothing mapped".to_string(),
            1 => "1 mapping".to_string(),
            n => format!("{n} mappings"),
        };
        ui.label(RichText::new(count).small().color(theme::text::QUIET));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            cross(ui, 14.0).on_hover_text("Close the Controller Map").clicked()
        })
        .inner
    })
    .inner
}

/// What the panel says with nothing mapped.
fn empty(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let text = "Nothing in this patch follows MIDI yet. Turn on Map Mode, click a control, then move a knob or fader or hit a pad on your controller. Every mapping you make is listed here.";
    ui.add(egui::Label::new(RichText::new(text).color(theme::text::SECONDARY)).wrap());
    ui.add_space(4.0);
}

/// The columns' names.
fn header(ui: &mut egui::Ui) {
    let names = ["Source", "Module", "Control", "Range", "Hardware", "Pick up", "Invert", ""];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (name, width) in names.iter().zip(COLUMNS) {
            cell(ui, width, |ui| {
                ui.label(RichText::new(*name).small().color(theme::text::QUIET));
            });
        }
    });
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::background::GRID_MAJOR);
}

/// A column's cell, `width` wide, its contents centred down the row.
fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(egui::vec2(width, ROW_HEIGHT), Layout::left_to_right(Align::Center), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

/// A mapping's row, its edits joining `edits`. True while it's hovered.
fn draw_row(ui: &mut egui::Ui, row: &Row, edits: &mut Vec<Edit>) -> bool {
    let mapping = &row.mapping;
    let key = row.key;
    let macro_slot = mapping.macro_slot.is_some();
    let cc = mapping.source == MidiSource::Cc && !macro_slot;
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let response = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let [source, module, control, range, hardware, pick_up, invert, clear] = COLUMNS;
        cell(ui, source, |ui| {
            ui.label(RichText::new(&row.source).color(theme::signal::MIDI.lerp_to_gamma(theme::text::PRIMARY, 0.3)));
        });
        cell(ui, module, |ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, row.accent);
            match &row.module {
                Some(name) => ui.add(egui::Label::new(RichText::new(name).color(theme::text::PRIMARY)).truncate()),
                None => ui.label(RichText::new("(gone)").italics().color(theme::text::DISABLED)),
            };
        });
        cell(ui, control, |ui| {
            ui.add(egui::Label::new(RichText::new(&mapping.param_name).color(theme::text::SECONDARY)).truncate());
        });
        cell(ui, range, |ui| match &row.spec {
            Some(spec) if cc => {
                let limits = (spec.min, spec.max);
                let id = ui.scope_id().with(("controller_map_range", key));
                let low = range_field(ui, id.with("min"), mapping.min_value, spec);
                ui.label(RichText::new("–").color(theme::text::DISABLED));
                let high = range_field(ui, id.with("max"), mapping.max_value, spec);
                if low.is_some() || high.is_some() {
                    edits.push(Edit::Range(key, (low.unwrap_or(mapping.min_value), high.unwrap_or(mapping.max_value)), limits));
                }
            }
            _ => quiet(ui, if macro_slot { "the DAW's" } else { "" }),
        });
        let mut style = MidiStyle { encoding: mapping.encoding, jump: mapping.jump, invert: mapping.invert };
        let styled = style;
        cell(ui, hardware, |ui| {
            if cc && matches!(mapping.control, MidiControl::Knob | MidiControl::Select) {
                hardware_menu(ui, key, &mut style.encoding);
            } else {
                quiet(ui, if mapping.source == MidiSource::Note { "pad or key" } else { "" });
            }
        });
        cell(ui, pick_up, |ui| {
            if cc && mapping.control == MidiControl::Knob && style.encoding == MidiEncoding::Absolute {
                let mut pick = !style.jump;
                ui.checkbox(&mut pick, "").on_hover_text("The knob waits where it is until the hardware reaches it, rather than jumping to the hardware");
                style.jump = !pick;
            }
        });
        cell(ui, invert, |ui| {
            if cc {
                ui.checkbox(&mut style.invert, "").on_hover_text("The hardware's top is the bottom here");
            }
        });
        if style != styled {
            edits.push(Edit::Style(key, style));
        }
        cell(ui, clear, |ui| {
            if cross(ui, 12.0).on_hover_text("Clear this mapping").clicked() {
                edits.push(Edit::Clear(key));
            }
        });
    });
    let rect = response.response.rect.expand2(egui::vec2(4.0, 1.0));
    let hovered = ui.rect_contains_pointer(rect);
    // Hovered, it's lit as its control is on the canvas: a faint wash, and
    // a purple edge down its left
    if hovered {
        let edge = egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height()));
        let shapes = vec![
            egui::Shape::rect_filled(rect, 4.0, theme::signal::MIDI.gamma_multiply(0.06)),
            egui::Shape::rect_filled(edge, 1.0, theme::signal::MIDI),
        ];
        ui.painter().set(backdrop, egui::Shape::Vec(shapes));
    }
    hovered
}

/// A small cross to click, `size` across: brighter under the pointer.
fn cross(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let ink = if response.hovered() { theme::text::PRIMARY } else { theme::text::QUIET };
    let arm = rect.shrink(size * 0.22);
    let stroke = egui::Stroke::new(1.5, ink);
    ui.painter().line_segment([arm.left_top(), arm.right_bottom()], stroke);
    ui.painter().line_segment([arm.right_top(), arm.left_bottom()], stroke);
    response
}

/// A word or two in a cell with nothing to edit.
fn quiet(ui: &mut egui::Ui, text: &str) {
    if !text.is_empty() {
        ui.label(RichText::new(text).small().color(theme::text::DISABLED));
    }
}

/// One end of a knob's range, in its units: what it was set to, once
/// Enter or a click elsewhere leaves it. Text it can't read shows in red,
/// and sets nothing.
fn range_field(ui: &mut egui::Ui, id: egui::Id, value: f32, spec: &NumberSpec) -> Option<f32> {
    let format = spec.format();
    let text_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(id));
    let mut text = if focused { ui.data(|d| d.get_temp::<String>(text_id)) } else { None }.unwrap_or_else(|| format.format(value));
    let read = |text: &str| format.parse(text, value).map(|v| v.clamp(spec.min, spec.max)).map(|v| if spec.stepped { v.round() } else { v });
    let ink = if read(&text).is_some() { theme::text::PRIMARY } else { theme::accent::ERROR };
    let hint = format!("Type where it goes from, as the knob reads: {} to {}. Enter sets it", format.format(spec.min), format.format(spec.max));
    let mut output = egui::TextEdit::singleline(&mut text).id(id).desired_width(84.0).text_color(ink).show(ui);
    // Clicked into, it's all selected, so typing replaces it
    if output.response.gained_focus() {
        let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
        output.state.cursor.set_char_range(Some(all));
        output.state.store(ui.ctx(), id);
    }
    let response = output.response.response.on_hover_text(hint);
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(text_id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(text_id));
    }
    let tolerance = (spec.max - spec.min).abs() * 1e-6;
    response.lost_focus().then(|| read(&text)).flatten().filter(|typed| (typed - value).abs() > tolerance)
}

/// What the hardware is: a knob or fader with ends, or one of the two
/// kinds of endless encoder.
fn hardware_menu(ui: &mut egui::Ui, key: MapKey, encoding: &mut MidiEncoding) {
    let kinds = [
        (MidiEncoding::Absolute, "Knob or fader", "It has ends, and sends where it is"),
        (MidiEncoding::Relative64, "Encoder (64 ±)", "Endless: sends 65 for a step up, 63 for a step down"),
        (MidiEncoding::TwosComplement, "Encoder (1 / 127)", "Endless: sends 1 for a step up, 127 for a step down"),
    ];
    let shown = kinds.iter().find(|(kind, ..)| kind == encoding).map_or("", |(_, name, _)| name);
    egui::ComboBox::from_id_salt(("controller_map_hardware", key)).width(COLUMNS[4] - 8.0).selected_text(shown).show_ui(ui, |ui| {
        for (kind, name, tip) in kinds {
            ui.selectable_value(encoding, kind, name).on_hover_text(tip);
        }
    });
}

/// Map Mode, from here too, and a word on where the edits go. True when
/// Map Mode's button was clicked.
fn footer(ui: &mut egui::Ui, map_mode: bool) -> bool {
    ui.horizontal(|ui| {
        let keys = shortcuts::keys(ui.ctx(), "Ctrl+L");
        let text = if map_mode { "Leave Map Mode" } else { "Map Mode" };
        let button = egui::Button::selectable(map_mode, text).shortcut_text(keys);
        let clicked = ui.add(button).on_hover_text("Click controls on the canvas one after another, moving something on your controller for each").clicked();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Saved with the patch · hover a row to find its control").small().color(theme::text::QUIET));
        });
        clicked
    })
    .inner
}
