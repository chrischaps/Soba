//! Files kept only in the browser's tab, kept in IndexedDB too.
//!
//! A take recorded in the browser, a file trimmed or normalized there, or
//! one dropped in, has no folder to live in (see `sample_files`'s
//! [`DROPPED_PREFIX`](super::sample_files::DROPPED_PREFIX)). So each is
//! also written to the page's IndexedDB under its key, and read back from
//! there when a patch names it and the shelf doesn't have it: after a
//! reload, an autosave recovered, or another patch opened in between.
//!
//! The first time the full app finds itself the only copy open (once the
//! recovery prompt is answered), it lets go of every file the patch it has
//! open doesn't name, as the desktop tidies the takes a crash left behind.
//!
//! `index.html` holds the IndexedDB side (`sobaTabFiles`). A browser
//! without IndexedDB (some private windows) keeps nothing, as before.
//!
//! Compiled only for the web build.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::sample_files::LOADING;

/// Why a key the store doesn't hold can't be read.
const NOT_KEPT: &str = "it was kept only in a browser tab, and isn't here";

/// How reading one file stands.
enum Read {
    /// Asked for; IndexedDB hasn't answered yet.
    Pending,
    /// Here, until the shelf takes it.
    Got(Vec<u8>),
    /// Why it couldn't be read.
    Failed(String),
}

#[derive(Default)]
struct Store {
    /// Whether the keys the store held at the start have been listed
    /// (or found unlistable), so a key not in `keys` isn't there.
    listed: bool,
    /// Whether that listing has been asked for.
    listing: bool,
    /// Every key the store holds, as far as this tab knows.
    keys: HashSet<String>,
    reads: HashMap<String, Read>,
    /// The keys written this visit, which [`tidy`] leaves alone.
    kept: HashSet<String>,
    /// Whether the files nobody names have been let go this visit.
    tidied: bool,
}

thread_local! {
    // The page has one thread, so IndexedDB answers on the one that asks
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

/// Starts listing what the store holds, the first time it's wanted.
fn list(store: &mut Store) {
    if std::mem::replace(&mut store.listing, true) {
        return;
    }
    wasm_bindgen_futures::spawn_local(async {
        let keys = call("keys", &[]).await.map(|keys| keys_of(&keys));
        if let Err(e) = &keys {
            warn(&format!("Soba can't keep the tab's files across a reload: {e}"));
        }
        STORE.with_borrow_mut(|store| {
            store.keys.extend(keys.unwrap_or_default());
            store.listed = true;
        });
    });
}

/// The bytes of the file kept under `key`, or [`LOADING`] while they're
/// read. The first ask starts the read; keep asking (the editor does,
/// every frame) to collect it. Once collected they're the asker's: asked
/// again, the file is read again.
pub fn read(key: &str) -> Result<Vec<u8>, String> {
    STORE.with_borrow_mut(|store| {
        list(store);
        if !store.listed {
            return Err(LOADING.to_string());
        }
        if !store.keys.contains(key) {
            return Err(NOT_KEPT.to_string());
        }
        match store.reads.remove(key) {
            Some(Read::Got(bytes)) => Ok(bytes),
            Some(Read::Failed(e)) => {
                store.reads.insert(key.to_string(), Read::Failed(e.clone()));
                Err(e)
            }
            Some(Read::Pending) => {
                store.reads.insert(key.to_string(), Read::Pending);
                Err(LOADING.to_string())
            }
            None => {
                store.reads.insert(key.to_string(), Read::Pending);
                let owned = key.to_string();
                wasm_bindgen_futures::spawn_local(async move {
                    let read = match call("get", &[owned.as_str().into()]).await {
                        Ok(bytes) if bytes.is_instance_of::<js_sys::Uint8Array>() => {
                            Read::Got(js_sys::Uint8Array::new(&bytes).to_vec())
                        }
                        Ok(_) => Read::Failed(NOT_KEPT.to_string()),
                        Err(e) => Read::Failed(e),
                    };
                    STORE.with_borrow_mut(|store| store.reads.insert(owned, read));
                });
                Err(LOADING.to_string())
            }
        }
    })
}

/// Whether the store holds a file under `key`, as far as this tab knows.
pub fn has(key: &str) -> bool {
    STORE.with_borrow_mut(|store| {
        list(store);
        store.keys.contains(key)
    })
}

/// Keeps `bytes` under `key`, replacing whatever was there. Written in a
/// moment; a write that fails is only noted in the console, since the tab
/// still has the file.
pub fn keep(key: &str, bytes: Vec<u8>) {
    STORE.with_borrow_mut(|store| {
        list(store);
        store.keys.insert(key.to_string());
        store.kept.insert(key.to_string());
        store.reads.remove(key);
    });
    let key = key.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let bytes = js_sys::Uint8Array::from(bytes.as_slice());
        if let Err(e) = call("put", &[key.as_str().into(), bytes.into()]).await {
            warn(&format!("Soba couldn't keep {key} for a reload: {e}"));
        }
    });
}

/// Whether [`tidy`] has run this visit.
pub fn tidied() -> bool {
    STORE.with_borrow(|store| store.tidied)
}

/// Lets go of every kept file `named` doesn't name, unless another copy
/// of the app is open (it may be playing them). Once a visit.
pub fn tidy(named: HashSet<String>) {
    if STORE.with_borrow_mut(|store| std::mem::replace(&mut store.tidied, true)) {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let others_open = call_now("othersOpen").map(|open| open.as_bool() != Some(false)).unwrap_or(true);
        if others_open {
            return;
        }
        let Ok(keys) = call("keys", &[]).await else { return };
        for key in keys_of(&keys).into_iter().filter(|key| !named.contains(key)) {
            // One kept since the patch was looked at is wanted
            if STORE.with_borrow(|store| store.kept.contains(&key)) {
                continue;
            }
            STORE.with_borrow_mut(|store| {
                store.keys.remove(&key);
                store.reads.remove(&key);
            });
            if let Err(e) = call("remove", &[key.as_str().into()]).await {
                warn(&format!("Soba couldn't let go of {key}: {e}"));
            }
        }
    });
}

/// The strings in a JS array of keys.
fn keys_of(keys: &JsValue) -> Vec<String> {
    js_sys::Array::from(keys).iter().filter_map(|key| key.as_string()).collect()
}

/// `sobaTabFiles[method]`, from `index.html`.
fn method(name: &str) -> Result<(JsValue, js_sys::Function), String> {
    let window = web_sys::window().ok_or("no window")?;
    let files = js_sys::Reflect::get(&window, &"sobaTabFiles".into()).map_err(|_| "no store")?;
    let method = js_sys::Reflect::get(&files, &name.into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or("no store")?;
    Ok((files, method))
}

/// Calls a method that answers at once.
fn call_now(name: &str) -> Result<JsValue, String> {
    let (files, method) = method(name)?;
    method.call0(&files).map_err(describe)
}

/// Calls a method that answers with a promise, and waits for it.
async fn call(name: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let (files, method) = method(name)?;
    let promise = method.apply(&files, &args.iter().collect::<js_sys::Array>()).map_err(describe)?;
    JsFuture::from(js_sys::Promise::from(promise)).await.map_err(describe)
}

/// What went wrong, in words.
fn describe(error: JsValue) -> String {
    js_sys::Reflect::get(&error, &"message".into())
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "IndexedDB isn't available".to_string())
}

fn warn(message: &str) {
    web_sys::console::warn_1(&message.into());
}
