//! Menus that stay open until an item closes them with `ui.close()`, as
//! egui's did before 0.32. Soba's menus hold sliders, value fields, toggles
//! and chips you click more than once, so a click inside one mustn't close
//! it the way egui's own menus now do. A click outside, or Escape, still does.
//! Submenus opened inside one follow it.

use egui::containers::menu::{is_in_menu, MenuButton, MenuConfig};
use egui::{InnerResponse, IntoAtoms, Popup, PopupCloseBehavior, Response, Ui};

const STAY_OPEN: PopupCloseBehavior = PopupCloseBehavior::CloseOnClickOutside;

/// `response.context_menu`, staying open as its items are clicked.
pub fn context_menu(response: &Response, add_contents: impl FnOnce(&mut Ui)) -> Option<InnerResponse<()>> {
    Popup::context_menu(response).close_behavior(STAY_OPEN).show(add_contents)
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
