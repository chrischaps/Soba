//! Menus that stay open until an item closes them with `ui.close()`, as
//! egui's did before 0.32. Soba's menus hold sliders, value fields, toggles
//! and chips you click more than once, so a click inside one mustn't close
//! it the way egui's own menus now do. A click outside, or Escape, still does.
//! Submenus opened inside one follow it.

use egui::containers::menu::{is_in_menu, MenuButton, MenuConfig};
use egui::{InnerResponse, IntoAtoms, Popup, PopupCloseBehavior, Response, Ui};

const STAY_OPEN: PopupCloseBehavior = PopupCloseBehavior::CloseOnClickOutside;

/// `response.context_menu`, staying open as its items are clicked. It keeps
/// its open state under an id of its own: egui's is the one a dropdown keeps
/// its list under, and a context menu closes its popup on every left click,
/// so a dropdown with a right-click menu would shut as soon as it opened.
pub fn context_menu(response: &Response, add_contents: impl FnOnce(&mut Ui)) -> Option<InnerResponse<()>> {
    Popup::context_menu(response).id(response.id.with("context_menu")).close_behavior(STAY_OPEN).show(add_contents)
}

/// `ui.menu_button`, staying open as its items are clicked. Inside a menu
/// it's a submenu, which takes its root's behaviour.
pub fn menu_button<'a, R>(ui: &mut Ui, atoms: impl IntoAtoms<'a>, add_contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
    if is_in_menu(ui) {
        return ui.menu_button(atoms, add_contents);
    }
    let (response, inner) = MenuButton::new(atoms).config(MenuConfig::new().close_behavior(STAY_OPEN)).ui(ui, add_contents);
    InnerResponse::new(inner.map(|inner| inner.inner), response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{ComboBox, Event, PointerButton, RawInput};

    /// One frame of a dropdown carrying a right-click menu, as a module's
    /// Select row is drawn. Returns the dropdown's button.
    fn frame(ctx: &egui::Context, events: Vec<Event>) -> Response {
        let mut button = None;
        ctx.run_ui(RawInput { events, ..Default::default() }, |ui| {
            let mut choice = 0;
            let combo = ComboBox::from_id_salt("choice").selected_text("One").show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, 0, "One");
                ui.selectable_value(&mut choice, 1, "Two");
            });
            context_menu(&combo.response, |ui| {
                ui.label("Learn MIDI");
            });
            button = Some(combo.response);
        })
        .textures_delta
        .clear();
        button.expect("drawn")
    }

    #[test]
    fn test_a_dropdown_with_a_right_click_menu_opens_on_a_click() {
        let ctx = egui::Context::default();
        let at = frame(&ctx, Vec::new()).rect.center();
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&ctx, vec![Event::PointerMoved(at)]);
        frame(&ctx, vec![press(true)]);
        frame(&ctx, vec![press(false)]);
        let button = frame(&ctx, Vec::new());
        assert!(Popup::is_id_open(&ctx, button.id.with("popup")), "a click should leave the dropdown's list open");
        assert!(!Popup::is_id_open(&ctx, button.id.with("context_menu")));
    }

    #[test]
    fn test_a_right_click_on_a_dropdown_opens_its_menu_instead() {
        let ctx = egui::Context::default();
        let at = frame(&ctx, Vec::new()).rect.center();
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Default::default() };
        frame(&ctx, vec![Event::PointerMoved(at)]);
        frame(&ctx, vec![press(true)]);
        frame(&ctx, vec![press(false)]);
        let button = frame(&ctx, Vec::new());
        assert!(Popup::is_id_open(&ctx, button.id.with("context_menu")));
        assert!(!Popup::is_id_open(&ctx, button.id.with("popup")));
    }
}
