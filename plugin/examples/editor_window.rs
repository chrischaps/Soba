//! The plugin's editor in a window of its own, with no DAW: the app as the
//! plugin makes it, drawn through the same egui-baseview glue, at `scale`
//! as a host would set it (1.0 unless given). It closes itself after
//! `frames` frames (300 unless given), and says how it went.
//!
//!     cargo run -p soba-plugin --example editor_window -- [frames] [scale]

use baseview::gl::GlConfig;
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::EguiWindow;
use rtrb::RingBuffer;
use soba::app::SynthApp;

fn main() {
    let frames: u32 = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(300);
    let scale: f64 = std::env::args().nth(2).and_then(|n| n.parse().ok()).unwrap_or(1.0);
    let (_ui_midi, heard) = RingBuffer::new(1024);
    let (app, _processor) = SynthApp::hosted(48000.0, heard);
    let started = std::time::Instant::now();
    EguiWindow::open_blocking(
        WindowOpenOptions {
            title: String::from("Soba (plugin editor)"),
            size: Size::new(1280.0, 760.0),
            scale: WindowScalePolicy::ScaleFactor(scale),
            gl_config: Some(GlConfig { version: (3, 2), srgb: true, double_buffer: true, vsync: true, ..Default::default() }),
        },
        Default::default(),
        (app, 0u32),
        |ctx, _queue, _state| soba_plugin::prepare_context(ctx),
        move |ctx, queue, (app, drawn)| {
            ctx.request_repaint();
            app.draw_frame(ctx);
            *drawn += 1;
            if *drawn == frames {
                println!("drew {drawn} frames in {:.1} s", started.elapsed().as_secs_f32());
                queue.close_window();
            }
        },
    );
}
