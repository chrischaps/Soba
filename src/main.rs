//! Soba - A node-based modular audio synthesizer
//!
//! Entry point for the application: a window on the desktop, or a canvas
//! in the browser (`trunk serve`, see `index.html`).

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;
#[cfg(not(target_arch = "wasm32"))]
use soba::app::capture::CaptureConfig;
#[cfg(not(target_arch = "wasm32"))]
use soba::app::update::{resume, Resume};
use soba::app::SynthApp;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let test_tone = args.iter().any(|arg| arg == "--test-tone");
    // Filming the app: `soba <patch> --capture <script> --out <dir>`
    let capture = match CaptureConfig::from_args(&args) {
        Ok(capture) => capture,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };
    // A lesson to start, e.g. `soba --lesson first-sound` (see lessons/)
    let lesson = args.iter().position(|a| a == "--lesson").and_then(|i| args.get(i + 1)).cloned();
    // A patch file to open, e.g. `soba patches/lush-pad.json`
    // (skipping the values that follow options)
    let patch_path = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, arg)| {
            let after = &args[i - 1];
            !arg.starts_with("--") && !CaptureConfig::is_option(after) && after != resume::OPTION && after != "--lesson"
        })
        .map(|(_, arg)| PathBuf::from(arg));
    // Restarted after an update: the session the copy before handed over.
    // Taking it waits for that copy to close
    let resume = Resume::file_from_args(&args).and_then(|file| Resume::take(&file));

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 720.0])
        .with_min_inner_size([800.0, 600.0])
        .with_title("Soba")
        .with_icon(std::sync::Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/icon-256.png")).unwrap_or_default(),
        ));
    if let Some(capture) = &capture {
        viewport = viewport.with_inner_size(capture.size_in_points()).with_resizable(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        // A capture's window size is its own, not one to remember
        persist_window: capture.is_none(),
        // A folder of its own for the settings and autosave, so a test copy
        // (an update tried end to end, say) leaves the everyday one's alone
        persistence_path: std::env::var_os("SOBA_DATA_DIR").map(PathBuf::from),
        ..Default::default()
    };

    // Sends any Link Out's lanes to Seoul and any MIDI Out's notes to its
    // device; idle while the patch has neither
    soba::link::sender::start();
    // The Ableton Link session, on the network only while a Clock follows it
    #[cfg(feature = "ableton-link")]
    soba::ableton_link::start();

    eframe::run_native(
        "Soba",
        options,
        Box::new(move |cc| {
            soba::app::theme::install_fonts(&cc.egui_ctx);
            let mut app = SynthApp::new(test_tone);
            if resume.is_none() {
                app.open_on_launch(patch_path.as_deref());
            }
            match capture {
                Some(config) => {
                    if let Err(e) = app.start_capture(config) {
                        eprintln!("capture: {}", e);
                        std::process::exit(2);
                    }
                }
                None => {
                    app.restore_session(cc.storage);
                    if let Some(resume) = resume {
                        app.resume(resume);
                    }
                }
            }
            // Last, so the lesson's patch is the one on show
            if let Some(name) = &lesson {
                if !app.open_lesson_named(&cc.egui_ctx, name) {
                    eprintln!("no lesson called {name}: see lessons/");
                }
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;
    use soba::app::web;

    let document = web_sys::window().and_then(|w| w.document()).expect("a page to run in");
    let canvas = document
        .get_element_by_id("soba")
        .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .expect("index.html's canvas");

    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| {
                    soba::app::theme::install_fonts(&cc.egui_ctx);
                    let mut app = SynthApp::new(false);
                    let (patch, open) = (web::query_param("patch"), web::query_param("open"));
                    // A shared patch (`#p=1.…`, see persistence/patch_link.rs),
                    // taken out of the address so a reload picks up the
                    // session instead of opening the link again over it
                    let link = web::fragment().filter(|f| soba::persistence::patch_link::holds_patch(f));
                    let refused = link.as_deref().and_then(|link| {
                        web::forget_fragment();
                        app.open_from_link(link).err()
                    });
                    let opened = (link.is_some() && refused.is_none()) || app.open_from_address(patch.as_deref(), open.as_deref());
                    if !opened {
                        app.open_on_launch(None);
                    }
                    if let Some(why) = refused {
                        app.link_refused(&cc.egui_ctx, why);
                    }
                    // An embed is the page's patch, not a session to pick up
                    if patch.is_none() {
                        app.restore_session(cc.storage);
                    }
                    // `?lesson=first-sound` starts a lesson, over the session
                    if let Some(name) = web::query_param("lesson").filter(|_| patch.is_none()) {
                        app.open_lesson_named(&cc.egui_ctx, &name);
                    }
                    Ok(Box::new(app))
                }),
            )
            .await;

        // The page says it's loading until the app takes over, or why it couldn't
        if let Some(loading) = document.get_element_by_id("loading") {
            match started {
                Ok(()) => loading.remove(),
                Err(e) => {
                    web_sys::console::error_1(&e);
                    loading.set_inner_html("Soba couldn't start here: it needs WebGL 2 and WebAssembly. Try a recent Chrome, Firefox or Safari.");
                }
            }
        }
    });
}
