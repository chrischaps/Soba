//! Frame pacing for animation (#307).
//!
//! Something moving asks for the next frame with [`animate`] rather than
//! `request_repaint()`. In front, that's the next frame at once, which vsync
//! paces. Behind another window it's about 30 a second, half the work for a
//! window nobody is looking at closely. Nothing moving asks for nothing: a
//! stopped patch draws only when the player does something.
//!
//! egui measures `stable_dt` only after a frame asked for the next at once.
//! After one asked for it later, it guesses 1/60 s, which would run every
//! meter and glide at half speed in the background, so [`before_frame`] tells
//! it how long the wait really was.

use std::time::Duration;

use eframe::egui;

/// About 30 frames a second for a window behind another.
pub const UNFOCUSED: Duration = Duration::from_millis(33);

/// Where the time of a frame that asked for a later one is kept.
fn paced_id() -> egui::Id {
    egui::Id::unique("pace::paced_at")
}

/// How long an animation waits for its next frame: none in front, which
/// leaves it to vsync, and [`UNFOCUSED`] behind.
pub fn frame(focused: bool) -> Duration {
    if focused {
        Duration::ZERO
    } else {
        UNFOCUSED
    }
}

/// Ask for the next frame of an animation.
#[track_caller]
pub fn animate(ctx: &egui::Context) {
    let (focused, now) = ctx.input(|i| (i.focused, i.time));
    if focused {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(UNFOCUSED);
        ctx.data_mut(|data| data.insert_temp(paced_id(), now));
    }
}

/// Before a frame: if the last one asked for this one later, egui's guess of
/// the time between them is the time between them.
pub fn before_frame(ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    let paced_at = ctx.data_mut(|data| data.remove_temp::<f64>(paced_id()));
    if let (Some(at), Some(now)) = (paced_at, raw_input.time) {
        raw_input.predicted_dt = ((now - at) as f32).clamp(1.0 / 240.0, 0.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a frame at `time` that animates, and gives its stable_dt.
    fn frame_at(ctx: &egui::Context, time: f64, focused: bool) -> f32 {
        let mut input = egui::RawInput { time: Some(time), focused, ..Default::default() };
        before_frame(ctx, &mut input);
        let mut dt = 0.0;
        ctx.run_ui(input, |ui| {
            let ctx = ui.ctx();
            dt = ctx.input(|i| i.stable_dt);
            animate(ctx);
        }).textures_delta.clear();
        dt
    }

    #[test]
    fn behind_another_window_animation_keeps_its_speed() {
        let ctx = egui::Context::default();
        frame_at(&ctx, 0.0, false);
        // A frame 40 ms later is 40 ms later, not egui's 60 Hz guess
        assert!((frame_at(&ctx, 0.04, false) - 0.04).abs() < 1e-4);
        assert!((frame_at(&ctx, 0.075, false) - 0.035).abs() < 1e-4);
    }

    #[test]
    fn in_front_frames_come_at_once() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput { time: Some(0.0), focused: true, ..Default::default() };
        before_frame(&ctx, &mut input);
        let mut output = ctx.run_ui(input, |ui| animate(ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay, Duration::ZERO);
        assert!((frame_at(&ctx, 0.007, true) - 0.007).abs() < 1e-4);
    }
}
