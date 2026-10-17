//! A patch that opened without some of its parts isn't saved back over its
//! file without asking (#311). A newer Soba's modules and parameters don't
//! change the patch version, so an older Soba opens its patches, skipping
//! what it doesn't know. Saving over the file would then lose those parts
//! for good, so Save asks where instead, as Save As does.

use super::*;

impl SynthApp {
    /// The file Save writes without asking: the one the patch came from,
    /// unless loading it left something out. In the browser the path is
    /// only a name, and saving downloads.
    pub(super) fn quick_save_path(&self) -> Option<PathBuf> {
        self.current_patch_path.clone().filter(|_| !WEB && self.dropped_on_load.is_empty())
    }

    /// Says why Save is asking where, when it's because of what the file has.
    pub(super) fn explain_save_as(&mut self) {
        let Some(path) = self.current_patch_path.as_ref().filter(|_| !WEB) else { return };
        if self.dropped_on_load.is_empty() {
            return;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("The patch");
        let count = self.dropped_on_load.len();
        self.raise_notice(format!(
            "{name} has {count} {} this Soba couldn't open (made by a newer one?). Saving over it would lose {}, so choose where to save",
            if count == 1 { "part" } else { "parts" },
            if count == 1 { "it" } else { "them" },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{ConnectionData, NodeData};

    /// A folder of its own for one test's patch files.
    fn folder(test: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("soba-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A sine into a module from a newer Soba, saved at `path`.
    fn from_a_newer_soba(path: &Path) {
        let mut patch = Patch::new("Newer");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        patch.nodes.push(NodeData::new(2, "fx.from_the_future", (200.0, 0.0)));
        patch.connections.push(ConnectionData::new(1, "Out", 2, "In"));
        save_to_file(&patch, path).unwrap();
    }

    #[test]
    fn a_patch_that_lost_parts_on_opening_isnt_saved_over() {
        let folder = folder("lossy-load");
        let path = folder.join("Newer.json");
        from_a_newer_soba(&path);
        let before = std::fs::read_to_string(&path).unwrap();

        let mut app = SynthApp::new(false);
        app.open_file(&path);
        assert_eq!(app.current_patch_path.as_deref(), Some(path.as_path()), "it still knows its file");
        assert_eq!(app.dropped_on_load.len(), 2, "the module and its cable: {:?}", app.dropped_on_load);
        assert_eq!(app.quick_save_path(), None, "Save asks where");
        app.explain_save_as();
        assert!(app.notice.is_some(), "and says why");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // Once saved somewhere, that file has everything the patch does
        let copy = folder.join("Copy.json");
        app.saved_as(&copy);
        app.current_patch_path = Some(copy.clone());
        assert_eq!(app.quick_save_path(), Some(copy));

        // A new patch has no file to lose anything from
        app.open_file(&path);
        app.new_patch();
        assert!(app.dropped_on_load.is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_patch_that_opened_whole_saves_over_its_file() {
        let folder = folder("whole-load");
        let path = folder.join("Whole.json");
        let mut patch = Patch::new("Whole");
        patch.nodes.push(NodeData::new(1, "osc.sine", (0.0, 0.0)));
        save_to_file(&patch, &path).unwrap();

        let mut app = SynthApp::new(false);
        app.open_file(&path);
        assert!(app.dropped_on_load.is_empty(), "{:?}", app.dropped_on_load);
        assert_eq!(app.quick_save_path(), Some(path.clone()));
        app.explain_save_as();
        assert!(app.notice.is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn recovering_an_autosave_remembers_what_its_file_had() {
        let folder = folder("lossy-recover");
        let path = folder.join("Newer.json");
        from_a_newer_soba(&path);

        let mut app = SynthApp::new(false);
        app.open_file(&path);
        let patch = app.create_patch("Newer");
        let autosave = Autosave::new(&patch, Some(path.clone()), None).unwrap();
        let autosave = autosave.with_dropped(app.dropped_on_load.clone());
        // Through JSON, as it's kept between sessions
        let autosave: Autosave = serde_json::from_str(&serde_json::to_string(&autosave).unwrap()).unwrap();

        let mut next = SynthApp::new(false);
        next.recover(&autosave);
        assert_eq!(next.current_patch_path.as_deref(), Some(path.as_path()));
        assert_eq!(next.dropped_on_load, app.dropped_on_load);
        assert_eq!(next.quick_save_path(), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
