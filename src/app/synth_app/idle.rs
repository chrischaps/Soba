//! A stopped patch draws only when something changes (#307): left open, it
//! should cost next to nothing.

#[cfg(test)]
mod tests {
    use super::super::*;

    /// Draws a few seconds of frames, stopped and untouched, and says what
    /// still wants the next one soon after, if anything does.
    fn settled(app: &mut SynthApp, ctx: &egui::Context, from: f64) -> Option<String> {
        // Animation asks on every frame; a timer running out or a retry asks once
        const LAST: usize = 30;
        let soon = std::time::Duration::from_millis(250);
        let (mut delay, mut asking) = (std::time::Duration::ZERO, 0);
        for frame in 0..240 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
                time: Some(from + frame as f64 / 60.0),
                predicted_dt: 1.0 / 60.0,
                focused: true,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.draw_frame(ui));
            output.textures_delta.clear();
            delay = output.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay);
            asking = if delay < soon { asking + 1 } else { 0 };
        }
        // A notice fading out asks once, seconds ahead: that's not animation
        (asking >= LAST).then(|| {
            let causes: Vec<String> = ctx.repaint_causes().iter().map(|c| format!("{}:{}", c.file, c.line)).collect();
            format!("{delay:?} from {}", causes.join(", "))
        })
    }

    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_stopped_example_asks_for_no_frames() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut app = SynthApp::new();
        // Its first check waits ten seconds of the wall clock, not the
        // frames', and then goes out to the network
        app.updater.auto_check = false;
        let mut busy = Vec::new();
        for (i, example) in EXAMPLES.iter().enumerate() {
            app.open_example(example);
            assert!(!app.is_playing);
            if let Some(why) = settled(&mut app, &ctx, i as f64 * 10.0) {
                busy.push(format!("{}: {why}", example.name));
            }
        }
        assert!(busy.is_empty(), "stopped, these still draw all the time:\n{}", busy.join("\n"));
    }
}
