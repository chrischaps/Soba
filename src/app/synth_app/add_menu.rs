//! The add menu: a right-click, or a finger held still, on empty canvas.
//!
//! Categories down the left, and the open one's modules in a submenu beside
//! them. Opened by a finger (#251), the rows grow to finger height, a
//! submenu with no room on either side opens in the menu's place behind a
//! ‹ Back row, and the module under a resting finger tells what it does in a
//! footer, since a finger never hovers.

use web_time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, RichText};
use egui_node_graph2::NodeTemplateTrait;

use crate::dsp::ModuleCategory;
use crate::graph::{AllNodeTemplates, SynthNodeTemplate};
use crate::persistence::Section;
use super::framing;
use super::library::SavedModule;
use super::theme;
use super::{NewAnnotation, SynthApp};

/// How tall a row is for a finger, in points. A mouse's are about 22.
const FINGER_ROW: f32 = 40.0;

/// How wide a submenu's rows are for a finger, in points.
const FINGER_WIDTH: f32 = 190.0;

/// The gap between the menu and its submenu.
const GAP: f32 = 4.0;

/// Hover delay before switching submenus (in seconds)
const SUBMENU_HOVER_DELAY: f32 = 0.15;

/// How the open add menu was opened, and where its submenu went.
#[derive(Default)]
pub(super) struct AddMenu {
    /// Opened by a long press: finger-sized, and no hovering.
    pub(super) finger: bool,
    /// The open submenu is shown in the menu's place.
    drilled: bool,
}

impl AddMenu {
    pub(super) fn opened(finger: bool) -> Self {
        Self { finger, drilled: false }
    }
}

/// Where a submenu goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    Right,
    /// The screen ends first on the right, as it soon does on a phone.
    Left,
    /// No room on either side: in the menu's place.
    Instead,
}

fn place(menu: Rect, width: f32, screen: Rect) -> Place {
    if menu.right() + GAP + width <= screen.right() {
        Place::Right
    } else if menu.left() - GAP - width >= screen.left() {
        Place::Left
    } else {
        Place::Instead
    }
}

/// What was chosen this frame.
#[derive(Default)]
struct Picks {
    template: Option<SynthNodeTemplate>,
    saved: Option<SavedModule>,
    /// A kit profile to add a group of Pads for (#469), by name.
    pads_for: Option<String>,
    open_library: bool,
    annotation: Option<NewAnnotation>,
    zoom_to: Option<framing::Target>,
    close: bool,
    /// A category row was clicked or tapped, rather than hovered.
    tapped_category: bool,
}

impl SynthApp {
    /// Shows the add menu while it's open, and does what's chosen in it.
    pub(super) fn show_add_menu(&mut self, ctx: &egui::Context, menu_just_opened: bool) {
        let Some(menu_pos) = self.user_state.context_menu_pos else { return };
        let mut picks = Picks::default();
        let categories = AllNodeTemplates::by_category();
        let finger = self.add_menu.finger;
        let screen = ctx.content_rect();
        let was_open = self.user_state.context_menu_open_category;

        // Where each category's row starts, for its submenu to line up with
        let mut row_tops: Vec<f32> = Vec::new();
        // An area keeps the width it last had, and the separators fill it, so
        // the categories and a submenu in their place each keep their own
        let menu_id = egui::Id::unique("add_node_context_menu").with(self.add_menu.drilled);
        let menu_response = egui::Area::new(menu_id)
            .fixed_pos(menu_pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(120.0);
                    if finger {
                        ui.spacing_mut().interact_size.y = FINGER_ROW;
                    }
                    match self.user_state.context_menu_open_category.filter(|_| self.add_menu.drilled) {
                        Some(index) => self.drilled_menu(ui, &categories, index, &mut picks),
                        None => row_tops = self.category_menu(ui, &categories, &mut picks),
                    }
                });
            });
        let menu_rect = menu_response.response.rect;

        // A category just opened with no room beside the menu opens in its
        // place, but only for a click: a passing mouse mustn't swap the menu
        let submenu_id = egui::Id::unique("add_node_submenu");
        let submenu_width = ctx.memory(|m| m.area_rect(submenu_id)).map_or(160.0, |r| r.width());
        let opened = self.user_state.context_menu_open_category;
        if !self.add_menu.drilled && opened.is_some() && opened != was_open && place(menu_rect, submenu_width, screen) == Place::Instead {
            if picks.tapped_category {
                self.add_menu.drilled = true;
                ctx.request_repaint();
            } else {
                self.user_state.context_menu_open_category = was_open;
            }
        }

        // Show submenu for open category
        let mut submenu_rect: Option<Rect> = None;
        if let Some(index) = self.user_state.context_menu_open_category.filter(|_| !self.add_menu.drilled) {
            let top = row_tops.get(index).map_or(menu_rect.top(), |top| top - ctx.global_style().spacing.menu_margin.top as f32);
            let area = egui::Area::new(submenu_id).order(egui::Order::Foreground);
            let area = match place(menu_rect, submenu_width, screen) {
                Place::Left => area.pivot(egui::Align2::RIGHT_TOP).fixed_pos(Pos2::new(menu_rect.left() - GAP, top)),
                _ => area.fixed_pos(Pos2::new(menu_rect.right() + GAP, top)),
            };
            let submenu_response = area.show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    if finger {
                        ui.spacing_mut().interact_size.y = FINGER_ROW;
                    }
                    self.submenu_rows(ui, &categories, index, &mut picks);
                });
            });
            submenu_rect = Some(submenu_response.response.rect);

            // Keep submenu open if mouse is inside it
            if submenu_response.response.rect.contains(ctx.input(|i| i.pointer.hover_pos().unwrap_or_default())) {
                // Reset hover intent when mouse is in submenu
                self.user_state.context_menu_hover_intent = None;
            }
        }

        // Close menu on click outside
        // Near the bottom of the window the menu is moved up to fit, off the
        // click that opened it, which mustn't count as a click outside
        if ctx.input(|i| i.pointer.any_click()) && !menu_just_opened && !self.long_press.lifted() {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                // Check if click was outside both main menu and submenu
                let in_main_menu = menu_rect.contains(pos);
                let in_submenu = submenu_rect.is_some_and(|r| r.contains(pos));
                let picked = picks.template.is_some() || picks.annotation.is_some() || picks.saved.is_some() || picks.pads_for.is_some();
                if !in_main_menu && !in_submenu && !picked {
                    picks.close = true;
                }
            }
        }

        // Close menu on Escape
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            picks.close = true;
        }

        // Create node if a template was selected
        if let Some(template) = picks.template {
            self.add_module_at(template, menu_pos);
            picks.close = true;
        }
        if let Some(saved) = picks.saved {
            self.add_saved_module(&saved, menu_pos);
        }
        if let Some(name) = picks.pads_for {
            self.add_pads_for(&name, menu_pos);
        }
        if picks.open_library {
            self.open_library_folder();
        }
        match picks.annotation {
            Some(NewAnnotation::Frame) if !self.graph_state.selected_nodes.is_empty() => self.frame_selection(ctx),
            Some(NewAnnotation::Frame) => self.add_frame_at(menu_pos),
            Some(NewAnnotation::Note) => self.add_note_at(menu_pos),
            None => {}
        }
        if let Some(target) = picks.zoom_to {
            self.start_glide(ctx, target);
        }

        if picks.close {
            self.user_state.context_menu_pos = None;
            self.user_state.context_menu_open_category = None;
            self.user_state.context_menu_hover_intent = None;
            self.add_menu.drilled = false;
        }
    }

    /// The categories, then My Modules if anything's saved there, then the
    /// Library: the rows that open submenus.
    fn menu_rows(&self, categories: &[(ModuleCategory, Vec<SynthNodeTemplate>)]) -> Vec<(&'static str, Color32)> {
        categories
            .iter()
            .map(|(category, _)| (category.name(), category.color()))
            .chain((!self.my_modules.is_empty()).then_some(("My Modules", theme::module::GROUP)))
            .chain(std::iter::once(("Library", theme::module::GROUP)))
            .collect()
    }

    /// The menu itself: the categories, frames and notes, and zooming.
    /// Returns where each category's row starts.
    fn category_menu(&mut self, ui: &mut egui::Ui, categories: &[(ModuleCategory, Vec<SynthNodeTemplate>)], picks: &mut Picks) -> Vec<f32> {
        let finger = self.add_menu.finger;
        let mut row_tops = Vec::new();
        for (cat_index, (name, color)) in self.menu_rows(categories).into_iter().enumerate() {
            // Create category button with arrow indicator
            let button_text = RichText::new(format!("{}  \u{25B6}", name)).color(color);

            let response = ui.add(
                egui::Button::new(button_text)
                    .min_size(egui::vec2(110.0, 0.0))
                    .frame(false)
            );
            row_tops.push(response.rect.top());

            // Handle hover intent with delay
            if response.hovered() {
                let now = Instant::now();

                // Check if we're already tracking this category
                if let Some((tracked_cat, hover_start)) = self.user_state.context_menu_hover_intent {
                    if tracked_cat == cat_index {
                        // Same category - check if delay has passed
                        if hover_start.elapsed().as_secs_f32() >= SUBMENU_HOVER_DELAY {
                            self.user_state.context_menu_open_category = Some(cat_index);
                        }
                    } else {
                        // Different category - start new tracking
                        self.user_state.context_menu_hover_intent = Some((cat_index, now));
                    }
                } else {
                    // No tracking yet - start tracking
                    self.user_state.context_menu_hover_intent = Some((cat_index, now));

                    // If no submenu is open, open immediately
                    if self.user_state.context_menu_open_category.is_none() {
                        self.user_state.context_menu_open_category = Some(cat_index);
                    }
                }
            }

            // Handle click to immediately open/toggle
            if response.clicked() {
                self.user_state.context_menu_open_category = Some(cat_index);
                self.user_state.context_menu_hover_intent = None;
                picks.tapped_category = true;
            }
        }

        // Frames and notes, to explain the patch, on its top level
        let framing = !self.graph_state.selected_nodes.is_empty();
        let annotating = self.frames_allowed();
        if annotating {
            ui.separator();
        }
        for (label, kind, hint) in [
            ("Frame", NewAnnotation::Frame,
                if framing { "A titled backdrop around the selected modules (Ctrl+Shift+F)" }
                else { "A titled backdrop to group modules under (Ctrl+Shift+F frames the selection)" }),
            ("Note", NewAnnotation::Note, "A card of text. **Bold** for emphasis"),
        ].into_iter().filter(|_| annotating) {
            let response = ui.add(
                egui::Button::new(RichText::new(label).color(theme::text::SECONDARY))
                    .min_size(egui::vec2(110.0, 0.0))
                    .frame(false),
            );
            if response.hovered() {
                self.user_state.context_menu_open_category = None;
                self.user_state.context_menu_hover_intent = None;
            }
            if hint_unless(response, &super::super::shortcuts::keys(ui.ctx(), hint), finger).clicked() {
                picks.annotation = Some(kind);
                picks.close = true;
            }
        }

        // Bringing things into view: F, and Shift+F for all of it
        ui.separator();
        let selection = self.selection_target();
        let selected = selection != framing::Target::All;
        for (label, shortcut, target, hint) in [
            ("Zoom to Selection", "F", selection, "Bring the selected modules, frames and notes into view"),
            ("Zoom to Fit", if selected { "Shift+F" } else { "F" }, framing::Target::All,
                if self.user_state.level.is_some() { "Bring everything in this group into view" }
                else { "Bring the whole patch into view, frames and notes too" }),
        ].into_iter().filter(|item| selected || item.2 == framing::Target::All) {
            let response = ui.add(
                egui::Button::new(RichText::new(label).color(theme::text::SECONDARY))
                    .shortcut_text(shortcut)
                    .min_size(egui::vec2(110.0, 0.0))
                    .frame(false),
            );
            if response.hovered() {
                self.user_state.context_menu_open_category = None;
                self.user_state.context_menu_hover_intent = None;
            }
            if hint_unless(response, hint, finger).clicked() {
                picks.zoom_to = Some(target);
                picks.close = true;
            }
        }
        row_tops
    }

    /// A submenu in the menu's place: a way back, the category's name, and
    /// its rows.
    fn drilled_menu(&mut self, ui: &mut egui::Ui, categories: &[(ModuleCategory, Vec<SynthNodeTemplate>)], index: usize, picks: &mut Picks) {
        let back = ui.add(
            egui::Button::new(RichText::new("\u{2039}  Back").color(theme::text::SECONDARY))
                .min_size(egui::vec2(110.0, 0.0))
                .frame(false),
        );
        if back.clicked() {
            self.user_state.context_menu_open_category = None;
            self.user_state.context_menu_hover_intent = None;
            self.add_menu.drilled = false;
        }
        if let Some((name, color)) = self.menu_rows(categories).get(index) {
            ui.label(RichText::new(name.to_uppercase()).small().color(*color));
        }
        self.submenu_rows(ui, categories, index, picks);
    }

    /// The open category's modules, My Modules or the Library. They scroll
    /// if the screen's too short for them, and for a finger a footer says
    /// what the one held down does.
    fn submenu_rows(&mut self, ui: &mut egui::Ui, categories: &[(ModuleCategory, Vec<SynthNodeTemplate>)], index: usize, picks: &mut Picks) {
        let finger = self.add_menu.finger;
        let mut held: Option<String> = None;
        if finger {
            // Room left over for the menu's margins, a Back row and the footer
            let max_height = (ui.ctx().content_rect().height() - 160.0).max(FINGER_ROW * 3.0);
            egui::ScrollArea::vertical()
                .id_salt(("add_menu_rows", index))
                .max_height(max_height)
                .show(ui, |ui| {
                    // One width for every row, the whole of it to tap
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        ui.set_width(FINGER_WIDTH);
                        self.submenu_items(ui, categories, index, picks, &mut held);
                    });
                });
            footer(ui, held.as_deref());
        } else {
            // The Library outgrows a short window too: it scrolls, by wheel
            let max_height = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 16.0).max(200.0);
            egui::ScrollArea::vertical()
                .id_salt(("add_menu_rows", index))
                .max_height(max_height)
                .show(ui, |ui| self.submenu_items(ui, categories, index, picks, &mut held));
        }
    }

    fn submenu_items(
        &mut self,
        ui: &mut egui::Ui,
        categories: &[(ModuleCategory, Vec<SynthNodeTemplate>)],
        index: usize,
        picks: &mut Picks,
        held: &mut Option<String>,
    ) {
        let finger = self.add_menu.finger;
        if let Some((_category, templates)) = categories.get(index) {
            ui.set_min_width(100.0);
            for template in templates {
                let label = template.node_finder_label(&mut self.user_state);
                if row(ui, label.as_ref(), template.description(), finger, held).clicked() {
                    picks.template = Some(*template);
                    picks.close = true;
                }
            }
        } else if index == categories.len() && !self.my_modules.is_empty() {
            // My Modules: the groups saved there
            ui.set_min_width(120.0);
            for saved in &self.my_modules {
                let text = RichText::new(&saved.name).color(theme::module::GROUP);
                if row(ui, text, &saved.summary, finger, held).clicked() {
                    picks.saved = Some(saved.clone());
                    picks.close = true;
                }
            }
            ui.separator();
            if ui.button(RichText::new("Open folder").color(theme::text::SECONDARY)).clicked() {
                picks.open_library = true;
                picks.close = true;
            }
        } else if index == categories.len() + usize::from(!self.my_modules.is_empty()) {
            // The Library: ready-made groups, under their sections
            ui.set_min_width(140.0);
            for (n, section) in Section::ALL.into_iter().enumerate() {
                if n > 0 {
                    ui.add_space(4.0);
                }
                ui.label(RichText::new(section.name().to_uppercase()).small().color(theme::text::QUIET));
                for saved in self.library_modules.iter().filter(|m| m.library_group().is_some_and(|g| g.section == section)) {
                    let text = RichText::new(&saved.name).color(theme::module::GROUP);
                    if row(ui, text, &saved.summary, finger, held).clicked() {
                        picks.saved = Some(saved.clone());
                        picks.close = true;
                    }
                }
                if section == Section::Controllers {
                    pads_for_menu(ui, picks);
                }
            }
        }
    }
}

/// Pads for ▸: a group of Pads for each kit profile (#469), its pads
/// named and set as the device's parts, for its own sequencer to play.
fn pads_for_menu(ui: &mut egui::Ui, picks: &mut Picks) {
    let profiles = crate::hardware::known();
    let kits: Vec<&crate::hardware::Profile> = profiles.all().iter().map(|k| &k.profile).filter(|p| crate::hardware::instrument::is_kit(p)).collect();
    if kits.is_empty() {
        return;
    }
    let text = RichText::new("Pads for  \u{25B6}").color(theme::module::GROUP);
    crate::widgets::menu::menu_button(ui, text, |ui| {
        for kit in kits {
            let tip = format!("A Pad for each of the {}'s {} parts, named for them, for its own sequencer to play Soba", kit.name, kit.parts.len());
            if ui.button(&kit.name).on_hover_text(tip).clicked() {
                picks.pads_for = Some(kit.name.clone());
                picks.close = true;
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("A drum machine's tracks as Pads: one a part, each on its note and channel");
}

/// A row that adds something. A mouse reads what it is in a tooltip; a
/// finger, which never hovers, in the footer while it rests on the row.
fn row(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>, hint: &str, finger: bool, held: &mut Option<String>) -> egui::Response {
    let response = ui.button(text);
    if finger && response.is_pointer_button_down_on() {
        *held = Some(hint.to_string());
    }
    hint_unless(response, hint, finger)
}

/// The hint as a tooltip, except for a finger, where it would pop up under it.
fn hint_unless(response: egui::Response, hint: &str, finger: bool) -> egui::Response {
    if finger { response } else { response.on_hover_text(hint) }
}

/// What the module under the finger does, as the palette's footer does for
/// the highlighted one. Room is kept for three lines, so the menu doesn't
/// grow under the finger as it rests.
fn footer(ui: &mut egui::Ui, held: Option<&str>) {
    let width = ui.min_rect().width().max(160.0);
    ui.set_max_width(width);
    ui.add_space(4.0);
    let rect = ui.min_rect();
    ui.painter().hline(rect.x_range(), ui.cursor().top(), egui::Stroke::new(1.0_f32, theme::background::WIDGET_HOVERED));
    ui.add_space(6.0);
    let top = ui.cursor().top();
    match held {
        Some(hint) => ui.label(RichText::new(hint).small().color(theme::text::SECONDARY)),
        None => ui.label(RichText::new("Tap to add  ·  hold to read").small().color(theme::text::QUIET)),
    };
    let line = ui.text_style_height(&egui::TextStyle::Small) + ui.spacing().item_spacing.y;
    ui.add_space((3.0 * line - (ui.cursor().top() - top)).max(0.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(left: f32) -> Rect {
        Rect::from_min_size(Pos2::new(left, 100.0), egui::vec2(120.0, 300.0))
    }

    #[test]
    fn a_submenu_opens_right_then_left_then_in_place() {
        let phone = Rect::from_min_size(Pos2::ZERO, egui::vec2(390.0, 844.0));
        assert_eq!(place(menu(10.0), 160.0, phone), Place::Right);
        assert_eq!(place(menu(250.0), 160.0, phone), Place::Left);
        // Mid-screen: 284 + 164 overruns 390 on the right, 136 - 164 the left
        assert_eq!(place(menu(140.0), 160.0, phone), Place::Instead);
        let desktop = Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 900.0));
        assert_eq!(place(menu(140.0), 160.0, desktop), Place::Right);
    }
}
