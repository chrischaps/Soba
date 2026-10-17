//! A stopped patch draws only when something changes (#307): left open, it
//! should cost next to nothing.

#[cfg(test)]
mod tests {
    use super::super::*;

    /// Draws a few seconds of frames, stopped and untouched, and says what
    /// still wants the next one soon after, if anything does.
    fn settled(app: &mut SynthApp, ctx: &egui::Context, from: f64) -> Option<String> {
        let mut delay = std::time::Duration::ZERO;
        for frame in 0..240 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
                time: Some(from + frame as f64 / 60.0),
                predicted_dt: 1.0 / 60.0,
                focused: true,
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| app.draw_frame(ctx));
            delay = output.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay);
        }
        // A notice fading out asks once, seconds ahead: that's not animation
        (delay < std::time::Duration::from_millis(250)).then(|| {
            let causes: Vec<String> = ctx.repaint_causes().iter().map(|c| format!("{}:{}", c.file, c.line)).collect();
            format!("{delay:?} from {}", causes.join(", "))
        })
    }

    #[test]
    #[ignore = "opens the audio device; run with --ignored"]
    fn a_stopped_example_asks_for_no_frames() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut app = SynthApp::new(false);
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
