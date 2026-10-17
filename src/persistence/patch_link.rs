//! Patch links: a whole patch in one URL that plays in the browser.
//!
//! The patch's JSON, compact, is deflated and written in base64url into the
//! address's *fragment*: `soba.chaps.dev/#p=1.<data>`. A fragment
//! never reaches a server, so the patch stays between the person who shares
//! it and the people who open it. The `1.` is the link format's version, so
//! a link made today keeps opening after the format moves on.
//!
//! A link carries the patch and nothing else. Samples that ship with the
//! examples are named by their key (`example:samples/keys.wav`) and fetched
//! by the browser as the example's are; a patch that plays the user's own
//! files can't be shared this way, and [`ShareError::OwnFiles`] names them.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use super::sample_files::{self, EXAMPLE_PREFIX};
use super::{patch_from_json, Patch};

/// Where the full app plays in the browser. A link is this with `#p=…`.
pub const PLAY_URL: &str = "https://soba.chaps.dev/";

/// The fragment key a patch is kept under.
const KEY: &str = "p=";

/// The link format this build writes. Version 1 is deflated compact JSON.
const VERSION: u32 = 1;

/// How large a patch's JSON a link may inflate to, so a hostile link can't
/// fill the tab's memory. The largest example is under 300 KB.
const MAX_JSON: usize = 8 * 1024 * 1024;

/// How hard deflate tries: 10 is miniz's best.
const LEVEL: u8 = 10;

/// Why a patch couldn't be made into a link.
#[derive(Debug, Clone, PartialEq)]
pub enum ShareError {
    /// It plays files of the user's own, by these names, which a link
    /// can't carry.
    OwnFiles(Vec<String>),
    /// It couldn't be written as JSON.
    Json(String),
}

impl fmt::Display for ShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShareError::OwnFiles(names) => {
                let files = if names.len() == 1 { "its own file" } else { "its own files" };
                write!(f, "it plays {files} ({}), which a link can't carry", names.join(", "))
            }
            ShareError::Json(e) => write!(f, "{e}"),
        }
    }
}

/// The fragment for `patch`, `p=1.<data>`, without the `#`. Its samples
/// must be named by key, as the editor names them.
pub fn fragment(patch: &Patch) -> Result<String, ShareError> {
    let own = own_files(patch);
    if !own.is_empty() {
        return Err(ShareError::OwnFiles(own));
    }
    let json = serde_json::to_vec(patch).map_err(|e| ShareError::Json(e.to_string()))?;
    let deflated = miniz_oxide::deflate::compress_to_vec(&json, LEVEL);
    Ok(format!("{KEY}{VERSION}.{}", URL_SAFE_NO_PAD.encode(deflated)))
}

/// The whole link for `patch`: [`PLAY_URL`] and its [`fragment`].
pub fn link(patch: &Patch) -> Result<String, ShareError> {
    Ok(format!("{PLAY_URL}#{}", fragment(patch)?))
}

/// Whether an address's fragment (with or without its `#`) holds a patch.
pub fn holds_patch(fragment: &str) -> bool {
    fragment.trim_start_matches('#').split('&').any(|part| part.starts_with(KEY))
}

/// The fragment of `text` if it's a link to a patch (pasted, say),
/// without its `#`. Patch JSON on the clipboard isn't one, even with a
/// `#p=` in a note's text.
pub fn fragment_of(text: &str) -> Option<&str> {
    let text = text.trim();
    let web = text.starts_with("https://") || text.starts_with("http://");
    if !web || text.contains(char::is_whitespace) {
        return None;
    }
    let (_, fragment) = text.split_once('#')?;
    holds_patch(fragment).then_some(fragment)
}

/// The patch a link's fragment holds: `p=1.<data>`, with or without the
/// `#`, among any other `&`-separated parts. Its samples are named by key.
pub fn patch_from_fragment(fragment: &str) -> Result<Patch, String> {
    let value = fragment
        .trim_start_matches('#')
        .split('&')
        .find_map(|part| part.strip_prefix(KEY))
        .ok_or("the link holds no patch")?;
    let (version, data) = value.split_once('.').ok_or("the link is cut short")?;
    match version.parse::<u32>() {
        Ok(VERSION) => {}
        Ok(newer) if newer > VERSION => {
            return Err("it was made by a newer Soba: reload the page to get it".to_string())
        }
        _ => return Err("the link isn't one Soba made".to_string()),
    }
    // A link pasted from a chat may have picked up a stray space or newline
    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let deflated = URL_SAFE_NO_PAD
        .decode(data.trim_end_matches('='))
        .map_err(|_| "the link is damaged (was it cut short?)")?;
    let json = miniz_oxide::inflate::decompress_to_vec_with_limit(&deflated, MAX_JSON)
        .map_err(|_| "the link is damaged (was it cut short?)")?;
    let json = String::from_utf8(json).map_err(|_| "the link is damaged")?;
    patch_from_json(&json).map_err(|e| e.to_string())
}

/// The names of the files `patch` plays that don't ship with the app, in
/// the order they're met, each once.
pub fn own_files(patch: &Patch) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for file in patch.all_nodes().into_iter().filter_map(|node| node.file.as_deref()) {
        let name = sample_files::file_name(file).to_string();
        if !file.starts_with(EXAMPLE_PREFIX) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::sample_files::{resolve, SampleBase};
    use crate::persistence::{NodeData, EXAMPLES};

    /// An example as the editor holds it: its samples named by key.
    fn keyed(patch: &mut Patch) {
        for node in patch.all_nodes_mut() {
            if let Some(file) = &node.file {
                node.file = Some(resolve(file, SampleBase::Example));
            }
        }
    }

    #[test]
    fn every_example_round_trips() {
        let mut lengths = Vec::new();
        for example in EXAMPLES {
            let mut patch = example.patch().unwrap();
            keyed(&mut patch);
            let link = link(&patch).unwrap_or_else(|e| panic!("{}: {e}", example.name));
            let (_, fragment) = link.split_once('#').unwrap();
            assert_eq!(patch_from_fragment(fragment).unwrap(), patch, "{}", example.name);
            assert_eq!(patch_from_fragment(&format!("#{fragment}")).unwrap(), patch);
            lengths.push((link.len(), example.name));
        }
        // `cargo test --lib patch_link -- --nocapture` lists them, for the PR
        lengths.sort();
        for (length, name) in &lengths {
            println!("{:>7} chars  {name}", length);
        }
    }

    #[test]
    fn a_link_is_plain_url_text() {
        let patch = EXAMPLES[0].patch().unwrap();
        let link = link(&patch).unwrap();
        assert!(link.starts_with("https://soba.chaps.dev/#p=1."));
        assert!(link.chars().all(|c| c.is_ascii_alphanumeric() || ":/.#=_-".contains(c)), "{link}");
    }

    #[test]
    fn own_files_are_named_and_refused() {
        let mut patch = Patch::new("mine");
        for file in ["example:samples/keys.wav", "C:/Music/kick.wav", "dropped:take 1.wav", "D:/kick.wav"] {
            let mut node = NodeData::new(patch.nodes.len() as u64, "sampler", (0.0, 0.0));
            node.file = Some(file.to_string());
            patch.nodes.push(node);
        }
        assert_eq!(own_files(&patch), ["kick.wav", "take 1.wav"]);
        let refused = fragment(&patch).unwrap_err();
        assert_eq!(refused, ShareError::OwnFiles(vec!["kick.wav".into(), "take 1.wav".into()]));
        assert!(refused.to_string().contains("kick.wav, take 1.wav"));
    }

    #[test]
    fn other_fragment_parts_are_ignored() {
        let patch = EXAMPLES[0].patch().unwrap();
        let fragment = fragment(&patch).unwrap();
        assert!(holds_patch(&format!("#x=2&{fragment}")));
        assert_eq!(patch_from_fragment(&format!("x=2&{fragment}&y")).unwrap(), patch);
        assert!(!holds_patch("#top"));
        let link = link(&patch).unwrap();
        assert_eq!(fragment_of(&format!("  {link}\n")), Some(fragment.as_str()));
        assert_eq!(fragment_of("{\"text\": \"see https://x/#p=1.abc\"}"), None);
        assert_eq!(fragment_of("https://soba.chaps.dev/"), None);
    }

    #[test]
    fn spaces_picked_up_on_the_way_are_dropped() {
        let patch = EXAMPLES[0].patch().unwrap();
        let fragment = fragment(&patch).unwrap();
        let (head, tail) = fragment.split_at(fragment.len() / 2);
        assert_eq!(patch_from_fragment(&format!("{head}\n {tail}")).unwrap(), patch);
    }

    #[test]
    fn bad_links_say_why() {
        let patch = EXAMPLES[0].patch().unwrap();
        let fragment = fragment(&patch).unwrap();
        let cut = &fragment[..fragment.len() / 2];
        assert!(patch_from_fragment(cut).unwrap_err().contains("damaged"));
        assert!(patch_from_fragment("p=9.abc").unwrap_err().contains("newer Soba"));
        assert!(patch_from_fragment("p=x.abc").unwrap_err().contains("isn't one Soba made"));
        assert!(patch_from_fragment("p=1").unwrap_err().contains("cut short"));
        assert!(patch_from_fragment("#top").unwrap_err().contains("no patch"));
    }
}
