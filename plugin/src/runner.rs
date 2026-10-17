//! Keeps the app's frames going while the editor is closed.
//!
//! The app does more each frame than draw: it compiles edits into plans for
//! the audio thread, takes back the plans and samples the audio thread is
//! done with, and turns knobs to the host's macros. With no window to draw
//! in, a thread of the plugin's own runs those frames on a context that
//! nothing shows.
//!
//! The thread is the plugin's to stop: a host may unload the plugin's
//! library as soon as the plugin is gone, and a thread still running its
//! code then would crash the host. Dropping the [`Runner`] stops it and
//! waits for it.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{Shared, SobaParams};

/// How often the app's frames run without the editor.
const FRAME_EVERY: Duration = Duration::from_millis(40);

/// The size the app lays itself out at, without a window.
const SCREEN: egui::Vec2 = egui::vec2(1280.0, 720.0);

/// The running thread; dropping it stops the thread and waits for it.
pub struct Runner {
    /// Dropped to wake the thread and stop it.
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// Starts the runner for the plugin that owns `shared`.
pub fn start(shared: &Arc<Shared>, params: &Arc<SobaParams>) -> Runner {
    let (stop, stopped) = mpsc::channel::<()>();
    let shared = Arc::downgrade(shared);
    let params = Arc::downgrade(params);
    let thread = std::thread::Builder::new()
        .name("soba-plugin-runner".into())
        .spawn(move || {
            let ctx = egui::Context::default();
            crate::prepare_context(&ctx);
            let started = Instant::now();
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(FRAME_EVERY) {
                frame(&ctx, &shared, &params, started);
            }
        });
    let thread = match thread {
        Ok(thread) => Some(thread),
        Err(e) => {
            nih_plug::nih_log!("Soba couldn't start its runner: {e}");
            None
        }
    };
    Runner { stop: Some(stop), thread }
}

/// Runs one of the app's frames, unless the editor is drawing them.
fn frame(ctx: &egui::Context, shared: &Weak<Shared>, params: &Weak<SobaParams>, started: Instant) {
    let (Some(shared), Some(params)) = (shared.upgrade(), params.upgrade()) else { return };
    if shared.editor_open.load(Ordering::Acquire) {
        return;
    }
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Default::default(), SCREEN)),
        time: Some(started.elapsed().as_secs_f64()),
        ..Default::default()
    };
    let macros = params.macros();
    shared.with_app(|app| {
        app.follow_macros(&macros);
        // Laid out and thrown away: no one sees it
        let _ = ctx.run(input, |ctx| app.draw_frame(ctx));
    });
}

impl Drop for Runner {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
