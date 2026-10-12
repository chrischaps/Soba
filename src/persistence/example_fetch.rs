//! The examples' samples in the browser: fetched from beside the page the
//! first time a patch plays one, and kept for the rest of the visit.
//!
//! The web build leaves the WAVs out of the wasm (see
//! [`super::examples::EXAMPLE_SAMPLES`]), and `index.html` has trunk copy
//! `patches/samples/` into `dist/`, so `example:samples/fm-bell-c4.wav` is
//! at `samples/fm-bell-c4.wav`, relative to the page. The browser's cache
//! keeps it between visits.
//!
//! Compiled only for the web build.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::sample_files::LOADING;

/// How a sample's fetch stands.
enum Fetch {
    /// Asked for; the page's server hasn't answered yet.
    Pending,
    /// Here. Kept for the rest of the visit, like a compiled-in sample.
    Got(&'static [u8]),
    /// Why it couldn't be fetched.
    Failed(String),
}

thread_local! {
    // The page has one thread, so the fetches answer on the one that asks
    static FETCHES: RefCell<HashMap<&'static str, Fetch>> = RefCell::new(HashMap::new());
}

/// The sample at `path` (as in `EXAMPLE_SAMPLES`): its bytes once fetched,
/// or [`LOADING`] while it's on its way. The first ask starts the fetch;
/// keep asking (the editor does, every frame) to collect it.
pub fn fetch(path: &'static str) -> Result<&'static [u8], String> {
    FETCHES.with_borrow_mut(|fetches| match fetches.get(path) {
        Some(Fetch::Got(bytes)) => Ok(*bytes),
        Some(Fetch::Failed(e)) => Err(e.clone()),
        Some(Fetch::Pending) => Err(LOADING.to_string()),
        None => {
            fetches.insert(path, Fetch::Pending);
            wasm_bindgen_futures::spawn_local(async move {
                let fetched = match get(path).await {
                    Ok(bytes) => Fetch::Got(Box::leak(bytes.into_boxed_slice())),
                    Err(e) => Fetch::Failed(e),
                };
                FETCHES.with_borrow_mut(|fetches| fetches.insert(path, fetched));
            });
            Err(LOADING.to_string())
        }
    })
}

/// Fetches a file from beside the page.
async fn get(path: &str) -> Result<Vec<u8>, String> {
    let unreachable = |_| "no connection".to_string();
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(path)).await.map_err(unreachable)?.unchecked_into();
    if !response.ok() {
        return Err(format!("fetch failed ({})", response.status()));
    }
    let body = JsFuture::from(response.array_buffer().map_err(unreachable)?).await.map_err(unreachable)?;
    Ok(js_sys::Uint8Array::new(&body).to_vec())
}
