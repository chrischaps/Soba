//! The top of a knob's right-click menu: its value, to type over in the
//! units its readout uses, and the way back to its default.

use egui::text::{CCursor, CCursorRange};
use egui::RichText;
use egui_node_graph2::NodeId;

use super::value_types::NumberSpec;
use crate::app::theme;

/// The id a knob's value field keeps its text under while the menu's open.
pub fn value_field_id(node_id: NodeId, param_name: &str) -> egui::Id {
    egui::Id::new((node_id, param_name, "knob value"))
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
    let current = format.format(value);
    let (mut text, fresh) = ui.data_mut(|d| d.get_temp::<(String, bool)>(id)).unwrap_or_else(|| (current.clone(), true));
    let read = |text: &str| format.parse(text, value).map(|v| v.clamp(spec.min, spec.max)).map(|v| if spec.stepped { v.round() } else { v });
    let mut chosen = None;
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Value").small().color(theme::text::SECONDARY));
            let edit_id = id.with("edit");
            let ink = if read(&text).is_some() { theme::text::PRIMARY } else { theme::accent::ERROR };
            let mut output = egui::TextEdit::singleline(&mut text).id(edit_id).desired_width(90.0).text_color(ink).show(ui);
            if fresh {
                if focus && enabled {
                    output.response.request_focus();
                }
                let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
                output.state.cursor.set_char_range(Some(all));
                output.state.store(ui.ctx(), edit_id);
            }
            let hint = format!("Type a value and press Enter: {} to {}", format.format(spec.min), format.format(spec.max));
            let response = output.response.response.on_hover_text(hint);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some(typed) = read(&text) {
                    chosen = Some(typed);
                    ui.close();
                }
            }
        });
        let at_default = (value - spec.default).abs() <= (spec.max - spec.min).abs() * 1e-6;
        let reset = ui
            .add_enabled(!at_default, egui::Button::new(format!("Reset to {}", format.format(spec.default))))
            .on_hover_text("Double-clicking the knob resets it too, as does Home once it's clicked");
        if reset.clicked() {
            chosen = Some(spec.default);
            ui.close();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, (text, false)));
    chosen
}
