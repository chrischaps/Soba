//! The two kinds of request the updater makes: one for the list of releases,
//! and one per download. Each says which app is asking (`User-Agent:
//! soba/<version>`, which GitHub requires) and nothing else: no
//! identifiers, no telemetry. They block, so they run on worker threads.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::release::{self, Release, REPO};

/// Overrides where releases are listed (a full `.../releases` API URL), to
/// try an update against a fork or a local server.
pub const URL_OVERRIDE: &str = "SOBA_UPDATE_URL";

/// The releases list to ask for.
pub fn releases_url() -> String {
    std::env::var(URL_OVERRIDE)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases?per_page=20"))
}

/// An agent whose response bodies must arrive within `body`. ureq times a
/// body as a whole, not read by read, so a download gets longer than a page.
fn agent(body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body))
        .user_agent(concat!("soba/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// How long the list of releases or a checksum file may take.
const PAGE: Duration = Duration::from_secs(30);
/// How long a release's zip may take: a backstop for a stalled connection,
/// long enough for a slow one.
const DOWNLOAD: Duration = Duration::from_secs(20 * 60);

const OFFLINE: &str = "couldn't reach GitHub (offline?)";

/// An I/O error that means the network or the server isn't there. ureq 3
/// reports a refused connection this way, not as `ConnectionFailed`.
fn unreachable(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::*;
    matches!(kind, ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | TimedOut | HostUnreachable | NetworkUnreachable)
}

/// Why a request didn't work, in words for the toast.
fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(403 | 429) => "GitHub is limiting requests from this network; try again in an hour".into(),
        ureq::Error::StatusCode(404) => "the releases weren't found".into(),
        ureq::Error::StatusCode(code) => format!("GitHub answered {code}"),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) => OFFLINE.into(),
        ureq::Error::Io(e) if unreachable(e.kind()) => OFFLINE.into(),
        other => other.to_string(),
    }
}

/// Every published release, newest first.
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    let url = releases_url();
    let body = agent(PAGE)
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(describe)?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("the answer was cut off: {e}"))?;
    release::parse_releases(&body).map_err(|e| e.to_string())
}

/// A small text file, such as `SHA256SUMS`.
pub fn fetch_text(url: &str) -> Result<String, String> {
    agent(PAGE).get(url).call().map_err(describe)?.body_mut().read_to_string().map_err(|e| e.to_string())
}

/// How far a download has got, shared with the UI.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    /// 0 until the size is known.
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    /// From 0 to 1, if the size is known.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32)
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The message a cancelled download ends with.
pub const CANCELLED: &str = "cancelled";

/// Downloads `url` into `dest`, returning its SHA-256 in lower-case hex.
/// `size` is what the release says, used when the server doesn't.
pub fn download(url: &str, dest: &Path, size: u64, progress: &Progress) -> Result<String, String> {
    let response = agent(DOWNLOAD).get(url).call().map_err(describe)?;
    let total = response.body().content_length().unwrap_or(size);
    progress.total.store(total, Ordering::Relaxed);

    let mut file = std::fs::File::create(dest).map_err(|e| format!("couldn't write {}: {e}", dest.display()))?;
    let mut reader = response.into_body().into_reader();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if progress.cancelled() {
            return Err(CANCELLED.into());
        }
        let n = reader.read(&mut buffer).map_err(|e| format!("the download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| format!("couldn't write the download: {e}"))?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.sync_all().map_err(|e| format!("couldn't write the download: {e}"))?;
    Ok(hex(&hasher.finalize()))
}

/// A file's SHA-256, in lower-case hex.
#[cfg(test)]
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_sha256sum() {
        let path = std::env::temp_dir().join("soba-sha256-test.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(path);
    }

    /// A server on a free local port that answers one request with
    /// `response`, and the URL to ask it at.
    fn answer_once(response: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                request.push(byte[0]);
            }
            stream.write_all(response).unwrap();
        });
        url
    }

    #[test]
    fn downloads_report_their_size_and_hash() {
        let url = answer_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc");
        let path = std::env::temp_dir().join("soba-download-test.bin");
        let progress = Progress::default();
        let hash = download(&url, &path, 0, &progress).unwrap();
        assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        assert_eq!(progress.fraction(), Some(1.0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_is_read_whole() {
        let url = answer_once(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nabc  a.zip\n");
        assert_eq!(fetch_text(&url).unwrap(), "abc  a.zip\n");
    }

    #[test]
    fn refusals_are_put_in_words() {
        let url = answer_once(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(fetch_text(&url).unwrap_err().starts_with("GitHub is limiting"));
        let url = answer_once(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert_eq!(fetch_text(&url).unwrap_err(), "GitHub answered 500");
    }

    #[test]
    fn nobody_listening_is_offline() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(fetch_text(&format!("http://127.0.0.1:{port}/")).unwrap_err(), "couldn't reach GitHub (offline?)");
    }
}
