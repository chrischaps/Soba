//! The patch as a document (#343): the file or example it came from, what
//! that file had that couldn't be opened, the files opened lately, and the
//! prompts that stand between it and losing changes ("Save changes?", and a
//! crash's autosave waiting to be recovered).
//!
//! Whether the patch has unsaved changes is the undo history's, the MIDI
//! mappings' and the loops' to say, so the app asks them and hands the
//! answer in where a name or title needs it.

use std::path::PathBuf;

use crate::persistence::{Example, Patch, EXAMPLES};
use super::session::{Autosave, Discard, RecentFiles};

/// The name of a patch that's never been saved, and isn't an example.
const UNTITLED: &str = "Untitled";

/// The open patch, as a document.
#[derive(Default)]
pub struct Document {
    /// The file the patch was opened from or last saved to. In the browser,
    /// only its name, to offer when it's saved again.
    pub path: Option<PathBuf>,
    /// The example the patch was opened from, until it's saved as a file or replaced.
    pub example: Option<&'static Example>,
    /// What loading the file left out (a newer Soba's modules, say), which
    /// saving over it would lose. See `lossy_load`.
    pub dropped_on_load: Vec<String>,
    /// Patch files opened or saved lately, for the Recent menu.
    pub recent: RecentFiles,
    /// What's waiting on an answer to "Save changes?".
    pub pending: Option<Discard>,
    /// A crash's autosave, until it's recovered or let go.
    pub recovery: Option<Autosave>,
    /// Whether the window may close: set once unsaved changes are dealt with.
    pub allow_close: bool,
    /// The window title last sent, so it's only sent when it changes.
    window_title: String,
}

impl Document {
    /// The patch is `path`'s, or with `None` no file's: not an example's.
    pub fn opened(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.example = None;
    }

    /// The patch is `example`'s, with no file.
    pub fn opened_example(&mut self, example: &'static Example) {
        self.path = None;
        self.example = Some(example);
    }

    /// A new patch: no file, no example, nothing left out.
    pub fn untitled(&mut self) {
        self.opened(None);
        self.dropped_on_load.clear();
    }

    /// The patch was saved to its file, `path`, which now holds all of it.
    pub fn saved(&mut self, path: PathBuf) {
        self.dropped_on_load.clear();
        self.recent.push(&path);
        self.opened(Some(path));
    }

    /// The patch's name: its file's, its example's, or "Untitled".
    pub fn title(&self) -> String {
        match (&self.path, self.example) {
            (Some(path), _) => path.file_stem().and_then(|s| s.to_str()).unwrap_or(UNTITLED).to_string(),
            (None, Some(example)) => example.name.to_string(),
            (None, None) => UNTITLED.to_string(),
        }
    }

    /// The status bar's name for the patch: its title, with an example
    /// marked as one. A new patch with nothing in it to lose has none.
    pub fn status_name(&self, unsaved: bool) -> Option<String> {
        match (&self.path, self.example) {
            (None, Some(example)) => Some(format!("{} (example)", example.name)),
            (None, None) if !unsaved => None,
            _ => Some(self.title()),
        }
    }

    /// The window's title: the patch, with a dot while it has unsaved
    /// changes. `Some` only when it isn't the title last asked for.
    pub fn new_window_title(&mut self, unsaved: bool) -> Option<String> {
        let title = format!("{}{} · Soba", if unsaved { "● " } else { "" }, self.title());
        if title == self.window_title {
            return None;
        }
        self.window_title = title.clone();
        Some(title)
    }

    /// The file name a save dialog offers: the file's own, or the example's.
    pub fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .or(self.example.map(|e| e.file_name))
            .unwrap_or("patch.json")
            .to_string()
    }

    /// Whether a prompt is up, so shortcuts wait.
    pub fn prompt_open(&self) -> bool {
        self.pending.is_some() || self.recovery.is_some()
    }

    /// `patch` kept as this document's autosave: its file, its example
    /// and what its file had that it doesn't.
    pub fn autosave(&self, patch: &Patch) -> Result<Autosave, String> {
        let example = self.example.map(|e| e.name.to_string());
        let autosave = Autosave::new(patch, self.path.clone(), example).map_err(|e| e.to_string())?;
        Ok(autosave.with_dropped(self.dropped_on_load.clone()))
    }

    /// The document an autosave was of, back: its file, its example and
    /// what its file had that it doesn't.
    pub fn recovered(&mut self, autosave: &Autosave) {
        self.path = autosave.path.clone();
        self.example = autosave.example.as_deref().and_then(|name| EXAMPLES.iter().find(|e| e.name == name));
        self.dropped_on_load = autosave.dropped.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_is_named_for_its_file_then_its_example() {
        let mut doc = Document::default();
        assert_eq!(doc.title(), "Untitled");
        assert_eq!(doc.status_name(false), None, "a new patch with nothing to lose");
        assert_eq!(doc.status_name(true).as_deref(), Some("Untitled"));
        assert_eq!(doc.file_name(), "patch.json");

        let example = &EXAMPLES[0];
        doc.opened_example(example);
        assert_eq!(doc.title(), example.name);
        assert_eq!(doc.status_name(false), Some(format!("{} (example)", example.name)));
        assert_eq!(doc.file_name(), example.file_name);

        doc.saved(PathBuf::from("songs").join("Night Drive.json"));
        assert_eq!(doc.title(), "Night Drive");
        assert_eq!(doc.status_name(false).as_deref(), Some("Night Drive"));
        assert_eq!(doc.file_name(), "Night Drive.json");
        assert!(doc.example.is_none());
    }

    #[test]
    fn saving_keeps_the_file_whole_and_recent() {
        let mut doc = Document::default();
        doc.opened(Some(PathBuf::from("a.json")));
        doc.dropped_on_load = vec!["a newer module".into()];
        doc.saved(PathBuf::from("b.json"));
        assert!(doc.dropped_on_load.is_empty());
        assert_eq!(doc.recent.iter().next(), Some(std::path::Path::new("b.json")));
        doc.dropped_on_load = vec!["x".into()];
        doc.untitled();
        assert!(doc.path.is_none() && doc.dropped_on_load.is_empty());
    }

    #[test]
    fn the_window_title_is_sent_when_it_changes() {
        let mut doc = Document::default();
        assert_eq!(doc.new_window_title(false).as_deref(), Some("Untitled · Soba"));
        assert_eq!(doc.new_window_title(false), None);
        assert_eq!(doc.new_window_title(true).as_deref(), Some("● Untitled · Soba"));
    }
}
