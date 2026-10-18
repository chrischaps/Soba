//! The patch Soba opens as it starts. A file named on the command line (or
//! double-clicked) joins the recent files, so the session holding them has
//! to be restored first: restoring replaces the list (#351).

use super::*;

impl SynthApp {
    /// Opens the patch the app starts with: the file named on the command
    /// line, or else the First Sound example, so the canvas is never empty.
    /// A relative path is made whole, so Recent and Save still find the file
    /// from wherever Soba starts next. Call it after `restore_session`.
    pub fn open_on_launch(&mut self, path: Option<&Path>) {
        match path {
            Some(path) => self.open_file(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())),
            None => self.open_example(examples::first_sound()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(std::collections::HashMap<String, String>);
    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn a_patch_opened_at_launch_joins_the_recent_files_whole() {
        let mut storage = Memory::default();
        let earlier = std::env::temp_dir().join("soba-earlier.json");
        let mut recent = RecentFiles::default();
        recent.push(&earlier);
        recent.store(&mut storage);

        // Relative to the working directory, as a command line gives it
        let relative = Path::new("patches/fm-synthesis.json");
        let mut app = SynthApp::new();
        app.restore_session(Some(&storage));
        app.open_on_launch(Some(relative));

        let whole = std::env::current_dir().unwrap().join(relative);
        let listed: Vec<&Path> = app.document.recent.iter().collect();
        assert_eq!(listed, [whole.as_path(), earlier.as_path()], "the launch patch on top of the session's");
        assert_eq!(app.document.path.as_deref(), Some(whole.as_path()));
    }
}
