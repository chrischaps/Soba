//! The ASIO Compatible logo, in builds with the `asio` feature.
//!
//! Steinberg's ASIO licence asks for its logo wherever ASIO is switched on
//! by hand, which in Soba is the Audio system list in the Output menu.

use eframe::egui::{self, RichText};

use super::theme;

/// Steinberg's logo, as supplied with the SDK (white on transparent).
const LOGO_PNG: &[u8] = include_bytes!("../../assets/asio/asio-compatible.png");

/// The logo's width in the menu, in points.
const LOGO_WIDTH: f32 = 44.0;

/// Draws the logo with the trademark notice beside it.
pub fn show(ui: &mut egui::Ui) {
    let Some(texture) = logo(ui.ctx()) else { return };
    let size = texture.size_vec2();
    ui.horizontal(|ui| {
        ui.add(egui::Image::new(&texture).fit_to_exact_size(egui::vec2(LOGO_WIDTH, LOGO_WIDTH * size.y / size.x)));
        ui.label(
            RichText::new("ASIO is a registered trademark of\nSteinberg Media Technologies GmbH")
                .color(theme::text::QUIET)
                .small(),
        );
    });
}

/// The logo as a texture, decoded the first time it's shown.
fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("asio_compatible_logo");
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let image = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    let texture = ctx.load_texture("asio_compatible_logo", pixels, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_logo_decodes() {
        let image = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).unwrap();
        assert!(image.width() > 0 && image.height() > 0);
    }
}
