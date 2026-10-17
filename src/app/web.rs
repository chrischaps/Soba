//! The browser's side of the app: files go out as downloads and come in
//! as uploads, and the page's address says what to open.
//!
//! Compiled only for the web build (`trunk build`, see `index.html`).

use std::sync::mpsc::Sender;

use eframe::egui;
use wasm_bindgen::JsCast;

/// A patch file the visitor picked to open.
pub struct Upload {
    /// The file's name, e.g. `lush-pad.json`.
    pub name: String,
    /// Its text, or why it couldn't be read.
    pub text: Result<String, String>,
}

/// Asks the visitor for a patch file. The browser's picker answers later:
/// the file arrives on `sender`, and a repaint is asked for to collect it.
pub fn pick_patch(ctx: &egui::Context, sender: Sender<Upload>) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new().add_filter("Soba Patch", &["json"]).pick_file().await else {
            return;
        };
        let text = String::from_utf8(file.read().await).map_err(|_| "it isn't a text file".to_string());
        let _ = sender.send(Upload { name: file.file_name(), text });
        ctx.request_repaint();
    });
}

/// Hands `text` to the browser to save as `file_name`, as a download.
pub fn download(file_name: &str, text: &str) -> Result<(), String> {
    download_bytes(file_name, text.as_bytes(), "application/json")
}

/// Hands `bytes` to the browser to save as `file_name`, as a download of
/// the media type `kind` (`audio/wav`).
pub fn download_bytes(file_name: &str, bytes: &[u8], kind: &str) -> Result<(), String> {
    let fail = |e: wasm_bindgen::JsValue| format!("{:?}", e);
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(kind);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(fail)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(fail)?;

    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let link: web_sys::HtmlAnchorElement = document.create_element("a").map_err(fail)?.unchecked_into();
    link.set_href(&url);
    link.set_download(file_name);
    link.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// The value of `key` in the page's address (`?patch=lush-pad`).
pub fn query_param(key: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    web_sys::UrlSearchParams::new_with_str(&search).ok()?.get(key)
}

/// The page address's fragment (`#p=1.…`), with its `#`.
pub fn fragment() -> Option<String> {
    web_sys::window()?.location().hash().ok().filter(|hash| !hash.is_empty())
}

/// Takes the fragment out of the page's address, without reloading it, so
/// reloading the page picks up the session rather than opening the link
/// again over it.
pub fn forget_fragment() {
    let Some(window) = web_sys::window() else { return };
    let location = window.location();
    let (Ok(path), Ok(search)) = (location.pathname(), location.search()) else { return };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&format!("{path}{search}")));
    }
}

/// The fragment the address took since it was last asked, if it changed:
/// a link pasted into the address bar of the tab that's already open,
/// which a browser doesn't reload for. `index.html` notes it.
pub fn take_new_fragment() -> Option<String> {
    let window = web_sys::window()?;
    let key = wasm_bindgen::JsValue::from_str("sobaNewHash");
    let hash = js_sys::Reflect::get(&window, &key).ok()?.as_string()?;
    let _ = js_sys::Reflect::delete_property(&window, &key);
    Some(hash)
}

/// The full app's address, without the query that makes a page an embed.
pub fn full_app_url() -> Option<String> {
    let location = web_sys::window()?.location();
    Some(format!("{}{}", location.origin().ok()?, location.pathname().ok()?))
}

/// Whether the browser is holding the page's sound until it's clicked
/// (its autoplay rule). `index.html` keeps track of the audio contexts.
pub fn audio_blocked() -> bool {
    let Some(window) = web_sys::window() else { return false };
    js_sys::Reflect::get(&window, &"sobaAudioBlocked".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .and_then(|f| f.call0(&window).ok())
        .and_then(|blocked| blocked.as_bool())
        .unwrap_or(false)
}
