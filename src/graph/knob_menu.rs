//! The top of a knob's right-click menu: its value, to type over in the
//! units its readout uses, and the way back to its default. An
//! Instrument's knob (#442) types in the device's own numbers (#466).

use egui::text::{CCursor, CCursorRange};
use egui::RichText;
use egui_node_graph2::NodeId;

use super::value_types::NumberSpec;
use crate::app::theme;
use crate::hardware::{instrument, Parameter, SlotMap};

/// The id a knob's value field keeps its text under while the menu's open.
pub fn value_field_id(node_id: NodeId, param_name: &str) -> egui::Id {
    egui::Id::unique((node_id, param_name, "knob value"))
}

/// Starts the value field afresh as the menu opens, from the knob's value
/// then. (Not while it's open: that would swallow the typing.)
pub fn reset_value_field(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.remove::<(String, bool)>(id));
}

/// The value field and Reset: the value either sets, if one does. The field
/// opens with the value selected, so typing replaces it, and takes the keys
/// when `focus` (not on a group's face, whose name field has them). Enter
/// sets what it reads; text it can't read shows in red. A knob a cable or
/// something else is in charge of shows them, but `enabled` false.
pub fn value_items(ui: &mut egui::Ui, id: egui::Id, spec: &NumberSpec, value: f32, enabled: bool, focus: bool) -> Option<f32> {
    let format = spec.format();
    let read = |text: &str| format.parse(text, value).map(|v| v.clamp(spec.min, spec.max)).map(|v| if spec.stepped { v.round() } else { v });
    let field = Field {
        current: format.format(value),
        read: &read,
        hint: format!("Type a value and press Enter: {} to {}", format.format(spec.min), format.format(spec.max)),
        steps: Vec::new(),
        default: spec.default,
        default_text: format.format(spec.default),
        at_default: (value - spec.default).abs() <= (spec.max - spec.min).abs() * 1e-6,
    };
    field.show(ui, id, enabled, focus)
}

/// The same for a knob holding a device's `parameter`, whose knob is at
/// `travel`: the field reads and takes what's sent as the device names
/// it, a number, a step's label or an offset ("+12"), and sets the knob to
/// send exactly that. Reset goes back to the chart's default, by name.
pub fn device_value_items(ui: &mut egui::Ui, id: egui::Id, parameter: &Parameter, travel: f32, enabled: bool, focus: bool) -> Option<f32> {
    let map = SlotMap::of(parameter);
    let read = |text: &str| parameter.parse(text).map(|value| map.travel(value));
    let start = instrument::start(parameter);
    let steps = parameter.steps();
    let hint = match (&steps[..], parameter.relative) {
        ([], false) => format!("Type a value and press Enter: {} to {}", parameter.range.0, parameter.range.1),
        ([], true) => format!("Type an offset and press Enter: {} to {}", parameter.text(parameter.range.0), parameter.text(parameter.range.1)),
        ([(first, _), .., (last, _)], _) => format!("Type a step and press Enter: {first} to {last}, or choose one below"),
        _ => "Type a step and press Enter".to_string(),
    };
    let field = Field {
        current: parameter.text(map.value(travel)),
        read: &read,
        hint,
        steps: steps.iter().map(|&(label, value)| (label.to_string(), map.travel(value))).collect(),
        default: start,
        default_text: parameter.text(map.value(start)),
        at_default: map.value(travel) == map.value(start),
    };
    field.show(ui, id, enabled, focus)
}

/// A value field, in whichever units its knob reads out.
struct Field<'a> {
    /// The knob's value as the field opens on it.
    current: String,
    /// The value text sets the knob to, if it reads.
    read: &'a dyn Fn(&str) -> Option<f32>,
    /// What the field takes, on hover.
    hint: String,
    /// For a knob of a few named values, each name and the value it sets,
    /// to choose from as well as type.
    steps: Vec<(String, f32)>,
    /// Where Reset sets it, and that as the readout writes it.
    default: f32,
    default_text: String,
    at_default: bool,
}

impl Field<'_> {
    fn show(self, ui: &mut egui::Ui, id: egui::Id, enabled: bool, focus: bool) -> Option<f32> {
        let (mut text, fresh) = ui.data_mut(|d| d.get_temp::<(String, bool)>(id)).unwrap_or_else(|| (self.current.clone(), true));
        let mut chosen = None;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Value").small().color(theme::text::SECONDARY));
                let edit_id = id.with("edit");
                let ink = if (self.read)(&text).is_some() { theme::text::PRIMARY } else { theme::accent::ERROR };
                let mut output = egui::TextEdit::singleline(&mut text).id(edit_id).desired_width(90.0).text_color(ink).show(ui);
                if fresh {
                    if focus && enabled {
                        output.response.request_focus();
                    }
                    let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
                    output.state.cursor.set_char_range(Some(all));
                    output.state.store(ui.ctx(), edit_id);
                }
                let response = output.response.response.on_hover_text(&self.hint);
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(typed) = (self.read)(&text) {
                        chosen = Some(typed);
                        ui.close();
                    }
                }
            });
            if !self.steps.is_empty() {
                crate::widgets::menu::menu_button(ui, "Steps", |ui| {
                    for (label, value) in &self.steps {
                        if ui.selectable_label(*label == self.current, label).clicked() {
                            chosen = Some(*value);
                            ui.close();
                        }
                    }
                });
            }
            let reset = ui
                .add_enabled(!self.at_default, egui::Button::new(format!("Reset to {}", self.default_text)))
                .on_hover_text("Double-clicking the knob resets it too, as does Home once it's clicked");
            if reset.clicked() {
                chosen = Some(self.default);
                ui.close();
            }
        });
        ui.data_mut(|d| d.insert_temp(id, (text, false)));
        chosen
    }
}
