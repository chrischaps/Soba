//! Short patch links: `soba.chaps.dev/s/<id>`, a name for a long link.
//!
//! The link service (`packaging/shortlinks/`, a Cloudflare Worker) keeps a
//! patch link's payload, `1.<data>` as it follows `#p=`, under a short ID
//! worked out from the patch itself, and hands it back. So a short link
//! always turns back into a long one, and opening it is opening that long
//! link (see [`super::patch_link`]). docs/SHORT_LINKS.md has the design.
//!
//! The short link is a static page on soba.chaps.dev (its `404.html`) that
//! turns `/s/<id>` into `/#s=<id>` for the app. The service's own address is
//! never shown to anyone, so it can move without breaking a link.
//!
//! Unlike a long link, a short one reaches a server, which keeps the patch.
//! Share says so, and keeps the long link on offer.

use std::sync::mpsc::{self, Receiver};

/// Where the link service answers.
pub const API: &str = "https://soba-links.chaps.workers.dev";

/// Overrides [`API`] on the desktop, to try a local `wrangler dev`.
#[cfg(not(target_arch = "wasm32"))]
pub const URL_OVERRIDE: &str = "SOBA_LINKS_URL";

/// What a short link is, before its ID.
pub const SHORT_URL: &str = "https://soba.chaps.dev/s/";

/// The fragment key a short link's ID is kept under, once the static page
/// has handed it to the app.
const KEY: &str = "s=";

/// What the service answered: the ID made, or the payload kept, or why not,
/// in words for a note.
pub type Answer = Result<String, String>;

/// Whether `id` could be one the service made: 6 to 16 letters and digits.
pub fn is_id(id: &str) -> bool {
    (6..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The short link for `id`.
pub fn link(id: &str) -> String {
    format!("{SHORT_URL}{id}")
}

/// The ID in an address's fragment (`#s=<id>`, with or without the `#`,
/// among any other `&`-separated parts).
pub fn id_in_fragment(fragment: &str) -> Option<&str> {
    fragment.trim_start_matches('#').split('&').find_map(|part| part.strip_prefix(KEY)).filter(|id| is_id(id))
}

/// The ID of `text` if it's a short link (pasted, say): the link itself,
/// `soba.chaps.dev/s/<id>`, or the app's address it becomes, `…/#s=<id>`.
pub fn id_of(text: &str) -> Option<&str> {
    let text = text.trim();
    if text.contains(char::is_whitespace) {
        return None;
    }
    let rest = text.strip_prefix("https://").or_else(|| text.strip_prefix("http://"))?;
    if let Some((_, fragment)) = rest.split_once('#') {
        return id_in_fragment(fragment);
    }
    let path = rest.strip_prefix("soba.chaps.dev/s/")?;
    let id = path.split(['?', '/']).next()?;
    is_id(id).then_some(id)
}

/// Asks the service for a short link to `payload` (`1.<data>`). The ID
/// arrives on the receiver, and `wake` is called when it has.
pub fn create(payload: String, wake: impl Fn() + Send + 'static) -> Receiver<Answer> {
    let (sender, receiver) = mpsc::channel();
    net::create(api(), payload, move |answer| {
        let _ = sender.send(answer);
        wake();
    });
    receiver
}

/// Asks the service for the payload kept under `id`, to open as a long
/// link's. It arrives on the receiver, and `wake` is called when it has.
pub fn resolve(id: String, wake: impl Fn() + Send + 'static) -> Receiver<Answer> {
    let (sender, receiver) = mpsc::channel();
    net::resolve(api(), id, move |answer| {
        let _ = sender.send(answer);
        wake();
    });
    receiver
}

fn api() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(url) = std::env::var(URL_OVERRIDE).ok().filter(|url| !url.trim().is_empty()) {
        return url.trim_end_matches('/').to_string();
    }
    API.to_string()
}

/// The ID in a created link's answer, `{"id": …, "url": …}`.
fn id_from_answer(body: &str) -> Answer {
    #[derive(serde::Deserialize)]
    struct Created {
        id: String,
    }
    serde_json::from_str::<Created>(body)
        .ok()
        .map(|created| created.id)
        .filter(|id| is_id(id))
        .ok_or_else(|| "the link service answered with something else".to_string())
}

/// Why the service said no, from its status.
fn refusal(status: u16) -> String {
    match status {
        404 => "no patch has that link (was it cut short?)".into(),
        410 => "it was taken down".into(),
        413 => "it's too large for a short link".into(),
        429 => "too many links at once: try again in a minute".into(),
        code => format!("the link service answered {code}"),
    }
}

/// The service didn't answer at all.
const UNREACHABLE: &str = "the link service didn't answer (offline?)";

/// The requests, on a worker thread: they block.
#[cfg(not(target_arch = "wasm32"))]
mod net {
    use std::time::Duration;

    use super::{id_from_answer, refusal, Answer, UNREACHABLE};

    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(8)))
            .user_agent(concat!("soba/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    }

    fn describe(error: ureq::Error) -> String {
        match error {
            ureq::Error::StatusCode(code) => refusal(code),
            _ => UNREACHABLE.into(),
        }
    }

    pub fn create(api: String, payload: String, answer: impl FnOnce(Answer) + Send + 'static) {
        std::thread::spawn(move || {
            let sent = agent()
                .post(&format!("{api}/v1/patches"))
                .header("Content-Type", "text/plain; charset=utf-8")
                .send(&payload);
            answer(sent.map_err(describe).and_then(|mut r| r.body_mut().read_to_string().map_err(|_| UNREACHABLE.into())).and_then(|b| id_from_answer(&b)));
        });
    }

    pub fn resolve(api: String, id: String, answer: impl FnOnce(Answer) + Send + 'static) {
        std::thread::spawn(move || {
            let got = agent().get(&format!("{api}/v1/patches/{id}")).call();
            answer(got.map_err(describe).and_then(|mut r| r.body_mut().read_to_string().map_err(|_| UNREACHABLE.into())));
        });
    }
}

/// The requests, through the page's `fetch`. A POST of plain text is a
/// simple request, so it needs no preflight.
#[cfg(target_arch = "wasm32")]
mod net {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{id_from_answer, refusal, Answer, UNREACHABLE};

    async fn fetch(url: String, body: Option<String>) -> Answer {
        let window = web_sys::window().ok_or(UNREACHABLE)?;
        let init = web_sys::RequestInit::new();
        if let Some(body) = &body {
            init.set_method("POST");
            init.set_body(&JsValue::from_str(body));
        }
        let response = JsFuture::from(window.fetch_with_str_and_init(&url, &init)).await.map_err(|_| UNREACHABLE)?;
        let response: web_sys::Response = response.unchecked_into();
        if !response.ok() {
            return Err(refusal(response.status()));
        }
        let text = JsFuture::from(response.text().map_err(|_| UNREACHABLE)?).await.map_err(|_| UNREACHABLE)?;
        text.as_string().ok_or_else(|| UNREACHABLE.to_string())
    }

    pub fn create(api: String, payload: String, answer: impl FnOnce(Answer) + 'static) {
        wasm_bindgen_futures::spawn_local(async move {
            let body = fetch(format!("{api}/v1/patches"), Some(payload)).await;
            answer(body.and_then(|b| id_from_answer(&b)));
        });
    }

    pub fn resolve(api: String, id: String, answer: impl FnOnce(Answer) + 'static) {
        wasm_bindgen_futures::spawn_local(async move { answer(fetch(format!("{api}/v1/patches/{id}"), None).await) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_links_are_read_from_the_address_and_from_a_paste() {
        assert_eq!(id_in_fragment("#s=haVBqXhw"), Some("haVBqXhw"));
        assert_eq!(id_in_fragment("x=1&s=haVBqXhw"), Some("haVBqXhw"));
        assert_eq!(id_in_fragment("#p=1.AAAA"), None);
        // Not one the service could have made
        assert_eq!(id_in_fragment("#s=abc"), None);
        assert_eq!(id_in_fragment("#s=../etc/passwd"), None);

        assert_eq!(id_of("https://soba.chaps.dev/s/haVBqXhw"), Some("haVBqXhw"));
        assert_eq!(id_of("  https://soba.chaps.dev/s/haVBqXhw/\n"), Some("haVBqXhw"));
        assert_eq!(id_of("http://soba.chaps.dev/s/haVBqXhw?utm=chat"), Some("haVBqXhw"));
        assert_eq!(id_of("https://soba.chaps.dev/#s=haVBqXhw"), Some("haVBqXhw"));
        assert_eq!(id_of("https://elsewhere.dev/s/haVBqXhw"), None);
        assert_eq!(id_of("https://soba.chaps.dev/#p=1.AAAA"), None);
        assert_eq!(id_of("look: https://soba.chaps.dev/s/haVBqXhw"), None);
        assert_eq!(id_of(r#"{"name": "s=haVBqXhw"}"#), None);
        assert_eq!(link("haVBqXhw"), "https://soba.chaps.dev/s/haVBqXhw");
    }

    #[test]
    fn the_answers_are_read_or_refused_in_words() {
        assert_eq!(id_from_answer(r#"{"id":"haVBqXhw","url":"https://soba.chaps.dev/s/haVBqXhw"}"#), Ok("haVBqXhw".into()));
        assert!(id_from_answer(r#"{"id":"<script>"}"#).is_err());
        assert!(id_from_answer("<html>").is_err());
        assert!(refusal(410).contains("taken down"));
        assert!(refusal(404).contains("no patch"));
    }

    /// The answer `receiver` gets, or a timeout.
    fn wait(receiver: Receiver<Answer>) -> Answer {
        receiver.recv_timeout(std::time::Duration::from_secs(15)).expect("an answer")
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_service_that_isnt_there_says_so_quickly() {
        // Nothing listens on the discard port
        let (sender, receiver) = mpsc::channel();
        let started = std::time::Instant::now();
        net::create("http://127.0.0.1:9".into(), "1.AAAA".into(), move |a| sender.send(a).unwrap());
        assert_eq!(wait(receiver), Err(UNREACHABLE.to_string()));
        assert!(started.elapsed().as_secs() < 5);
    }

    /// Against a running `npx wrangler dev` in packaging/shortlinks, or
    /// wherever SOBA_LINKS_URL says. Four examples, each sent twice, stay
    /// inside the service's ten new links a minute; its own test
    /// (`spike_test.mjs`) sends every example.
    #[test]
    #[ignore = "needs the link service; run with --ignored"]
    #[cfg(not(target_arch = "wasm32"))]
    fn examples_go_by_short_link_and_come_back_the_same() {
        if std::env::var(URL_OVERRIDE).is_err() {
            std::env::set_var(URL_OVERRIDE, "http://127.0.0.1:8787");
        }
        use crate::persistence::sample_files::{resolve as key, SampleBase};
        use crate::persistence::{patch_link, EXAMPLES};
        let chosen = ["First Sound", "Sampled Keys", "From One Sine", "Canopy"];
        for example in EXAMPLES.iter().filter(|e| chosen.contains(&e.name)) {
            // As the editor holds it: its samples named by key
            let mut patch = example.patch().unwrap();
            for node in patch.all_nodes_mut() {
                if let Some(file) = &node.file {
                    node.file = Some(key(file, SampleBase::Example));
                }
            }
            let fragment = patch_link::fragment(&patch).unwrap();
            let payload = fragment.strip_prefix("p=").unwrap().to_string();
            let id = wait(create(payload.clone(), || {})).unwrap();
            // The same patch, the same link
            assert_eq!(wait(create(payload.clone(), || {})).unwrap(), id, "{}", example.name);
            let back = wait(resolve(id.clone(), || {})).unwrap();
            assert_eq!(back, payload, "{}", example.name);
            assert_eq!(patch_link::patch_from_fragment(&format!("p={back}")).unwrap(), patch, "{}", example.name);
            println!("{:>22}  {:>6} chars -> {}", example.name, payload.len(), link(&id));
        }
        assert!(wait(resolve("zzzzzzzz".into(), || {})).unwrap_err().contains("no patch"));
    }
}
