//! The browser build's audio load and dropouts in the status bar (#374).
//! On the web, cpal renders each buffer on the page's main thread, after
//! whatever frame is being drawn, so a buffer can come out late while the
//! sound itself is cheap. The DSP time the engine reports can't see that;
//! the page measures it (`index.html`'s `sobaAudioLoad`) and the meter
//! shows the page's measure instead.

use super::*;

/// The page's late buffers: how many, and whether one came lately.
#[derive(Clone, Copy, Debug)]
pub(super) struct Dropouts {
    pub late: u64,
    pub recent: bool,
}

impl SynthApp {
    /// Points the CPU meter at the page's load and returns its dropouts,
    /// or None if the page has no measure (the desktop build).
    pub(super) fn poll_page_audio(&mut self) -> Option<Dropouts> {
        #[cfg(target_arch = "wasm32")]
        {
            let load = crate::app::web::audio_load()?;
            let now = Instant::now();
            self.cpu_load = load.average;
            self.cpu_peak.feed(load.peak, now);
            let recent = self.audio.output_dropped(load.late, now);
            Some(Dropouts { late: load.late, recent })
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
}

/// The dropout count beside the meter: amber for a while after each one,
/// quiet after that, and absent until there's been one.
pub(super) fn dropouts_label(ui: &mut egui::Ui, dropouts: Dropouts) {
    if dropouts.late == 0 {
        return;
    }
    let plural = if dropouts.late == 1 { "" } else { "s" };
    let (text, color) = if dropouts.recent {
        (format!("⚠ {} dropout{plural}", dropouts.late), theme::accent::WARNING)
    } else {
        (format!("{} dropout{plural}", dropouts.late), theme::text::SECONDARY)
    };
    ui.label(RichText::new(text).color(color).small()).on_hover_text(format!(
        "Buffers that came out late: {}\n\
         The page was busy drawing as a buffer came due, so the sound stopped for a moment. \
         A lighter patch, a smaller window or the desktop app keeps it steady.",
        dropouts.late
    ));
    if dropouts.recent {
        ui.ctx().request_repaint_after(INPUT_GLITCH_HOLD);
    }
}
