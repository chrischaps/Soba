//! The plugin's editor: the app, drawn in a window the DAW gives it.
//!
//! nih_plug_egui does this for egui 0.31; Soba is on 0.30, so this is the
//! same glue over egui-baseview's egui 0.30 release, with the app's frame
//! as the editor's update.

use baseview::gl::GlConfig;
use baseview::{Size, WindowHandle, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::EguiWindow;
use nih_plug::prelude::{Editor, GuiContext, ParentWindowHandle};
use parking_lot::Mutex;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{Shared, SobaParams};

/// The editor's size, in logical pixels: the app's own window's.
const SIZE: (u32, u32) = (1280, 760);

pub struct SobaEditor {
    shared: Arc<Shared>,
    params: Arc<SobaParams>,
    /// The scale the host asked for, if any. On macOS the system's is used.
    scale: Mutex<Option<f32>>,
}

impl SobaEditor {
    pub fn new(shared: Arc<Shared>, params: Arc<SobaParams>) -> Self {
        let scale = if cfg!(target_os = "macos") { None } else { Some(1.0) };
        Self { shared, params, scale: Mutex::new(scale) }
    }
}

/// The host's window handle, in the raw-window-handle baseview takes.
struct Parent(ParentWindowHandle);

unsafe impl HasRawWindowHandle for Parent {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = raw_window_handle::XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

impl Editor for SobaEditor {
    fn spawn(&self, parent: ParentWindowHandle, _context: Arc<dyn GuiContext>) -> Box<dyn std::any::Any + Send> {
        let shared = self.shared.clone();
        let params = self.params.clone();
        let scale = *self.scale.lock();
        let window = EguiWindow::open_parented(
            &Parent(parent),
            WindowOpenOptions {
                title: String::from("Soba"),
                size: Size::new(SIZE.0 as f64, SIZE.1 as f64),
                scale: scale.map(|s| WindowScalePolicy::ScaleFactor(s as f64)).unwrap_or(WindowScalePolicy::SystemScaleFactor),
                gl_config: Some(GlConfig {
                    version: (3, 2),
                    red_bits: 8,
                    blue_bits: 8,
                    green_bits: 8,
                    alpha_bits: 8,
                    depth_bits: 24,
                    stencil_bits: 8,
                    samples: None,
                    srgb: true,
                    double_buffer: true,
                    vsync: true,
                    ..Default::default()
                }),
            },
            Default::default(),
            (),
            |ctx, _queue, _state| crate::prepare_context(ctx),
            move |ctx, _queue, _state| {
                // The app asks for repaints as it needs them, but a host
                // may show the window before it's mapped: draw regardless
                ctx.request_repaint();
                let macros = params.macros();
                shared.with_app(|app| {
                    app.follow_macros(&macros);
                    app.draw_frame(ctx);
                });
            },
        );
        self.shared.editor_open.store(true, Ordering::Release);
        Box::new(EditorHandle { shared: self.shared.clone(), window })
    }

    fn size(&self) -> (u32, u32) {
        SIZE
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // Not while open: there's no rescaling a window baseview has made
        if self.shared.editor_open.load(Ordering::Acquire) {
            return false;
        }
        *self.scale.lock() = Some(factor);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}

    fn param_values_changed(&self) {}
}

/// The open editor; closing it hands the frames back to the runner.
struct EditorHandle {
    shared: Arc<Shared>,
    window: WindowHandle,
}

// baseview's handle holds raw pointers; the host only drops it on its GUI thread
unsafe impl Send for EditorHandle {}

impl Drop for EditorHandle {
    fn drop(&mut self) {
        self.shared.editor_open.store(false, Ordering::Release);
        self.window.close();
    }
}
