//! Plant Soba's examples in the Patch Garden, so it's never empty.
//!
//! ```text
//! cargo run --release --bin garden_seed [-- --dry-run]
//! ```
//!
//! Each example is opened in the app, without an audio device, and encoded
//! as Share encodes it ([`SynthApp::shared_examples`]). A short link's ID is
//! a hash of that encoding, so encoding the example files themselves, or
//! encoding them in another language, would plant twins of the links people
//! later share from the app. Examples that play files of their own are
//! skipped, as Share would refuse them.
//!
//! The plants go to the live link service, or wherever `SOBA_LINKS_URL`
//! says (`http://127.0.0.1:8787` for `npm run dev` in packaging/shortlinks).
//! They're sent one every twelve seconds, five a minute, well inside the
//! service's ten, so all of them take five minutes or so. They're planted
//! last example first: the garden shows the newest first, so it then reads
//! in the Examples menu's order.
//!
//! Every plant starts `pending`. The last line printed is the command that
//! shows them all (`npm run garden:show -- <id> …`). Planting again is
//! harmless: the same patch answers with the same ID and changes nothing.

use std::process::ExitCode;
use std::time::Duration;

use soba::app::synth_app::SynthApp;
use soba::persistence::short_link::{self, API, URL_OVERRIDE};

/// Between one plant and the next: five a minute.
const PACE: Duration = Duration::from_secs(12);

/// How long to wait when the service says too many, and how many times.
const BACK_OFF: Duration = Duration::from_secs(60);
const TRIES: usize = 3;

/// Who planted them.
const NAME: &str = "Soba";

fn main() -> ExitCode {
    let dry_run = std::env::args().skip(1).any(|arg| arg == "--dry-run");
    let api = std::env::var(URL_OVERRIDE)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .map_or(API.to_string(), |url| url.trim().trim_end_matches('/').to_string());

    // Each example as Share sends it, last first
    let mut seeds = Vec::new();
    for (name, shared) in SynthApp::shared_examples().into_iter().rev() {
        match shared {
            Ok(seed) => seeds.push(seed),
            Err(e) => println!("{name:>22}  skipped: {e}"),
        }
    }

    println!("{} examples to plant at {api}", seeds.len());
    if dry_run {
        for (title, payload) in &seeds {
            println!("{title:>22}  {:>6} chars", payload.len());
        }
        return ExitCode::SUCCESS;
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent(concat!("soba-garden-seed/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut planted = Vec::new();
    let mut failed = 0;
    for (index, (title, payload)) in seeds.iter().enumerate() {
        if index > 0 {
            std::thread::sleep(PACE);
        }
        let body = serde_json::json!({ "payload": payload, "title": title, "name": NAME }).to_string();
        match plant(&agent, &api, &body) {
            Ok(id) => {
                println!("{title:>22}  {}", short_link::link(&id));
                planted.push(id);
            }
            Err(why) => {
                println!("{title:>22}  not planted: {why}");
                failed += 1;
            }
        }
    }

    if !planted.is_empty() {
        let local = if api.contains("127.0.0.1") || api.contains("localhost") { " --local" } else { "" };
        println!("\nTo show them in the garden, in packaging/shortlinks:");
        println!("npm run garden:show -- {}{local}", planted.join(" "));
    }
    if failed > 0 {
        eprintln!("{failed} not planted");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Plants one, waiting a minute and trying again while the service says
/// too many. The plant's ID, or why not.
fn plant(agent: &ureq::Agent, api: &str, body: &str) -> Result<String, String> {
    for _ in 0..TRIES {
        let mut response = agent
            .post(&format!("{api}/v1/plants"))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| format!("the link service didn't answer ({e})"))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        match status {
            200..=299 => {
                #[derive(serde::Deserialize)]
                struct Created {
                    id: String,
                }
                return serde_json::from_str::<Created>(&text)
                    .ok()
                    .map(|created| created.id)
                    .filter(|id| short_link::is_id(id))
                    .ok_or_else(|| format!("the link service answered with something else: {text}"));
            }
            429 => {
                println!("{:>22}  too many at once: waiting a minute", "");
                std::thread::sleep(BACK_OFF);
            }
            _ => return Err(format!("{status} {}", text.trim())),
        }
    }
    Err("still too many after waiting".into())
}
