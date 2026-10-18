//! A window out of sight still plays (#381).
//!
//! Since egui 0.34, eframe calls `App::ui` only while the window can be
//! seen: not while it's minimised, or wholly covered on a Mac. Before, the
//! frame that drew the patch also drained the engine's events, heard MIDI,
//! finished takes and shipped edits to the audio thread, so all of that
//! would stop with the drawing. [`SynthApp::tend_out_of_sight`] runs from
//! `App::logic`, which eframe calls every pass, and does those parts of the
//! frame while nothing is drawn.

use std::time::Duration;

use eframe::egui;

use super::{Discard, SynthApp};

/// How often a window out of sight looks in while it plays: often enough
/// for the engine's events and a finishing take, with nothing to animate.
const OUT_OF_SIGHT: Duration = Duration::from_millis(100);

impl SynthApp {
    /// The parts of a frame that aren't drawing, while the window can't be
    /// seen. While it can, [`SynthApp::draw_frame`] does them.
    pub(super) fn tend_out_of_sight(&mut self, ctx: &egui::Context) {
        // eframe's own test: a window it can't tell about is drawn
        if ctx.input(|i| i.viewport().visible()) != Some(false) {
            return;
        }
        self.process_engine_events();
        self.recover_from_driver_reset();
        self.follow_default_output(ctx);
        // CC-mapped knobs keep turning, and Learn keeps listening
        self.process_midi_events(ctx.input(|i| i.time));
        self.poll_recording(ctx);
        self.tend_takes();

        // Quitting from the taskbar with unsaved changes brings the window
        // back, so the question can be seen
        if ctx.input(|i| i.viewport().close_requested()) && !self.document.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.request(ctx, Discard::Quit);
        }

        let behind = self.ui_handle.as_mut().is_some_and(|handle| !handle.flush());
        if self.is_playing || self.recording.is_some() || behind {
            ctx.request_repaint_after(OUT_OF_SIGHT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with no audio device, as the plugin's begins. Not
    /// `SynthApp::hosted`: that marks the whole test process a plugin's, and
    /// a Clock in another test would then offer the host, not Link (#403).
    fn app() -> SynthApp {
        let mut app = SynthApp::with_devices(Err(crate::engine::AudioError::NoOutputDevice), None);
        app.audio.error = None;
        app
    }

    /// A context with the app's fonts, as `main` makes it, but not its theme.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::app::theme::install_fonts(&ctx);
        ctx
    }

    /// One pass as eframe runs it, `minimized` or not: the logic, then the
    /// drawing only while the window can be seen. Returns how soon it asks
    /// for the next.
    fn pass(app: &mut SynthApp, ctx: &egui::Context, minimized: bool) -> Duration {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0))),
            ..Default::default()
        };
        let info = egui::ViewportInfo { minimized: Some(minimized), occluded: Some(false), ..Default::default() };
        input.viewports.insert(egui::ViewportId::ROOT, info);
        let mut output = ctx.run_ui(input, |ui| {
            app.tend_out_of_sight(ui.ctx());
            if !minimized {
                app.draw_frame(ui);
            }
        });
        output.textures_delta.clear();
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    }

    /// The Ui eframe hands the first frame was made before the app's theme
    /// was applied, and the frame still draws in it (the Title style is the
    /// theme's).
    #[test]
    fn the_first_frame_draws_in_a_fresh_context() {
        pass(&mut app(), &context(), false);
    }

    /// Playing while minimised, the app keeps looking in, without drawing.
    #[test]
    fn a_minimised_window_looks_in_while_it_plays() {
        let ctx = context();
        let mut app = app();
        pass(&mut app, &ctx, false);
        // The frames egui asks for after the first, before it settles
        for _ in 0..3 {
            pass(&mut app, &ctx, true);
        }
        app.is_playing = true;
        // Less a frame's predicted time: not at once, and not after OUT_OF_SIGHT
        let delay = pass(&mut app, &ctx, true);
        assert!(delay > Duration::ZERO && delay <= OUT_OF_SIGHT, "{delay:?}");
        app.is_playing = false;
        assert!(pass(&mut app, &ctx, true) > OUT_OF_SIGHT, "stopped, it rests");
    }

    /// In sight, the frame does all of it: nothing is done twice.
    #[test]
    fn a_window_in_sight_is_left_to_the_frame() {
        let ctx = context();
        let mut app = app();
        app.is_playing = true;
        // egui asks for a second frame after the first, and a third for the
        // window's theme it sends in the first (since 0.37)
        let mut delay = Duration::ZERO;
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.tend_out_of_sight(ui.ctx()));
            output.textures_delta.clear();
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        }
        assert_eq!(delay, Duration::MAX);
    }
}
