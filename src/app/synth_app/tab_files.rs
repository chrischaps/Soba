//! Files kept only in the browser's tab, and how they leave it.
//!
//! In the browser there's no folder beside the patch: a take recorded into
//! a Sampler, or a file trimmed or normalized, lives in the tab, as a file
//! dropped there does (see `takes`). The Sampler's file menu offers it as a
//! download. A patch downloaded names each such file by its name alone, as
//! one beside it, so downloaded into the same folder as its takes it opens
//! whole on the desktop. Downloading a patch says which files to fetch.

use std::sync::Arc;

use egui_node_graph2::NodeId;

use crate::persistence::sample_files;
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
                self.status_message = Some(format!("Couldn't download {name}: it isn't loaded"));
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            self.status_message = Some(match super::web::download_bytes(&name, &sample_files::encode_wav(&source), "audio/wav") {
                Ok(()) => format!("Downloaded {name}"),
                Err(e) => format!("Couldn't download {name}: {e}"),
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        drop(source);
    }
}

/// What to say once the patch `file_name` is downloaded, naming the files
/// it plays that are kept only in the tab, if any.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(super) fn downloaded(file_name: &str, in_tab: &[String]) -> String {
    let fetch = "into the same folder, and it opens whole in Soba on the desktop";
    match in_tab {
        [] => format!("Downloaded {file_name}"),
        [one] => format!("Downloaded {file_name}. It plays {one}, kept only in this tab: download that from its Sampler's name too, {fetch}"),
        many => format!(
            "Downloaded {file_name}. It plays {} files kept only in this tab ({}): download each from its Sampler's name too, {fetch}",
            many.len(),
            many.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_names_the_files_left_in_the_tab() {
        assert_eq!(downloaded("song.json", &[]), "Downloaded song.json");
        let one = downloaded("song.json", &["Take 1.wav".to_string()]);
        assert!(one.contains("It plays Take 1.wav, kept only in this tab"), "{one}");
        let two = downloaded("song.json", &["Take 1.wav".to_string(), "Take 2.wav".to_string()]);
        assert!(two.contains("2 files kept only in this tab (Take 1.wav, Take 2.wav)"), "{two}");
    }
}
