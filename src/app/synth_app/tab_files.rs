//! Files kept only in the browser's tab, and how they leave it.
//!
//! In the browser there's no folder beside the patch: a take recorded into
//! a Sampler, or a file trimmed or normalized, lives in the tab, as a file
//! dropped there does (see `takes`). The Sampler's file menu offers it as a
//! download. A patch that plays such files downloads as a zip instead:
//! `<name>.json` beside `<name> samples/`, the files in it as WAVs and the
//! patch naming them there. Unzipped, it opens whole on the desktop, the way
//! Save As gathers a patch's samples beside it there.
//!
//! Each is kept in the browser's IndexedDB too (`persistence::tab_store`),
//! so a reload, an autosave recovered, or the downloaded patch opened
//! again in the browser, finds it. An embed keeps nothing.

use std::sync::Arc;

use egui_node_graph2::NodeId;

use crate::persistence::sample_files;
use crate::persistence::stored_zip::StoredZip;
use super::SynthApp;

impl SynthApp {
    /// Saves a copy of the file a Sampler plays, kept only in the tab, as a
    /// download. The desktop keeps every file on disk already, so it has
    /// nothing to download.
    pub(super) fn download_sample(&mut self, node_id: NodeId) {
        let Some(key) = self.graph_state.graph.nodes.get(node_id).and_then(|node| node.user_data.file.clone()) else { return };
        let name = sample_files::file_name(&key).to_string();
        let source = match self.user_state.samples.get(&key) {
            Some(Ok(shelved)) => Arc::clone(&shelved.source),
            _ => {
                self.status_error(format!("Couldn't download {name}: it isn't loaded"));
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            match super::web::download_bytes(&name, &sample_files::encode_wav(&source), "audio/wav") {
                Ok(()) => self.status_message = Some(format!("Downloaded {name}")),
                Err(e) => self.status_error(format!("Couldn't download {name}: {e}")),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        drop(source);
    }

    /// Saves `patch` as a download named `file_name`: the patch alone, or,
    /// when it plays files kept only in the tab, a zip of it with them.
    /// Returns what to say about it.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn download_patch_whole(&self, file_name: &str, mut patch: crate::persistence::Patch) -> Result<String, String> {
        let name = file_name.strip_suffix(".json").unwrap_or(file_name);
        let folder = format!("{name} samples");
        let in_tab = sample_files::name_tab_files_in(&mut patch, &folder);
        let json = crate::persistence::patch_to_json(&patch).map_err(|e| e.to_string())?;
        if in_tab.is_empty() {
            super::web::download(file_name, &json)?;
            return Ok(format!("Downloaded {file_name}"));
        }

        let now = js_sys::Date::new_0();
        let zip = StoredZip::new(
            now.get_full_year() as u16,
            now.get_month() as u8 + 1,
            now.get_date() as u8,
            now.get_hours() as u8,
            now.get_minutes() as u8,
            now.get_seconds() as u8,
        );
        let wav = |name: &str| match self.user_state.samples.get(&format!("{}{name}", sample_files::DROPPED_PREFIX)) {
            Some(Ok(shelved)) => Some(sample_files::encode_wav(&shelved.source)),
            _ => None,
        };
        let (bytes, missing) = patch_zip(zip, file_name, &json, &folder, &in_tab, wav)?;
        let zip_name = format!("{name}.zip");
        super::web::download_bytes(&zip_name, &bytes, "application/zip")?;
        Ok(downloaded(&zip_name, file_name, &folder, in_tab.len() - missing.len(), &missing))
    }

    /// Keeps a file kept only in the tab, made of `bytes`, in the browser's
    /// IndexedDB too, under its key. The desktop has it on disk already.
    pub(super) fn keep_in_tab(&self, key: &str, bytes: impl FnOnce() -> Vec<u8>) {
        #[cfg(target_arch = "wasm32")]
        if !self.embedded {
            crate::persistence::tab_store::keep(key, bytes());
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (key, bytes);
    }

    /// Once the recovery prompt is answered (or there was none), lets go of
    /// the files IndexedDB keeps that the patch now open doesn't name, as
    /// the desktop tidies the takes a crash left behind. Once a visit.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn tidy_tab_files(&self) {
        use crate::persistence::tab_store;
        if self.embedded || self.recovery.is_some() || tab_store::tidied() {
            return;
        }
        let patch = self.create_patch("");
        tab_store::tidy(patch.all_nodes().into_iter().filter_map(|node| node.file.clone()).collect());
    }
}

/// A zip of the patch `json`, as `file_name`, with each file `in_tab` in
/// `folder` beside it, encoded by `wav`. Returns its bytes, and the files
/// `wav` had nothing for (not loaded), left out.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn patch_zip(
    mut zip: StoredZip,
    file_name: &str,
    json: &str,
    folder: &str,
    in_tab: &[String],
    wav: impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<(Vec<u8>, Vec<String>), String> {
    zip.add(file_name, json.as_bytes())?;
    let mut missing = Vec::new();
    for name in in_tab {
        match wav(name) {
            Some(bytes) => zip.add(&format!("{folder}/{name}"), &bytes)?,
            None => missing.push(name.clone()),
        }
    }
    Ok((zip.finish()?, missing))
}

/// What to say once `zip_name` is downloaded: the patch `file_name` with
/// `kept` files in `folder`, and any `missing` that couldn't go with it.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn downloaded(zip_name: &str, file_name: &str, folder: &str, kept: usize, missing: &[String]) -> String {
    let files = if kept == 1 { "file".to_string() } else { format!("{kept} files") };
    let mut said = format!(
        "Downloaded {zip_name}: {file_name} with the {files} it plays from this tab, in “{folder}”. Unzipped, it opens whole in Soba on the desktop"
    );
    if !missing.is_empty() {
        said += &format!(". {} wasn't loaded, so it's left out", missing.join(", "));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_says_what_went_with_the_patch() {
        let one = downloaded("Song.zip", "Song.json", "Song samples", 1, &[]);
        assert_eq!(
            one,
            "Downloaded Song.zip: Song.json with the file it plays from this tab, in “Song samples”. Unzipped, it opens whole in Soba on the desktop"
        );
        let two = downloaded("Song.zip", "Song.json", "Song samples", 2, &["Take 3.wav".to_string()]);
        assert!(two.contains("with the 2 files it plays"), "{two}");
        assert!(two.ends_with(". Take 3.wav wasn't loaded, so it's left out"), "{two}");
    }

    #[test]
    fn a_patch_zips_with_its_files_in_its_samples_folder() {
        use std::io::Read;
        let in_tab = ["Take 1.wav".to_string(), "gone.wav".to_string()];
        let wav = |name: &str| (name == "Take 1.wav").then(|| b"RIFF....WAVE".to_vec());
        let (bytes, missing) =
            patch_zip(StoredZip::new(2026, 10, 15, 12, 0, 0), "Song.json", "{}", "Song samples", &in_tab, wav).unwrap();
        assert_eq!(missing, ["gone.wav"]);

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names: Vec<_> = archive.file_names().map(str::to_string).collect();
        names.sort();
        assert_eq!(names, ["Song samples/Take 1.wav", "Song.json"]);
        let mut take = Vec::new();
        archive.by_name("Song samples/Take 1.wav").unwrap().read_to_end(&mut take).unwrap();
        assert_eq!(take, b"RIFF....WAVE");
    }
}
