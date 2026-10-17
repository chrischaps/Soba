//! The patch as the plugin's state, kept in the DAW's project.

use nih_plug::params::persist::PersistentField;
use std::sync::{Arc, Weak};

use crate::Shared;

/// The patch, read from the app when the host saves and opened in it when
/// the host restores. A host that restores before the plugin is set up
/// leaves it waiting in [`Shared`] until the app is made.
pub struct PatchState {
    shared: Weak<Shared>,
}

impl PatchState {
    pub fn new(shared: &Arc<Shared>) -> Self {
        Self { shared: Arc::downgrade(shared) }
    }
}

impl<'a> PersistentField<'a, String> for PatchState {
    fn set(&self, state: String) {
        let Some(shared) = self.shared.upgrade() else { return };
        if state.is_empty() {
            return;
        }
        let opened = shared.with_app(|app| app.restore_hosted_state(&state));
        match opened {
            Some(Ok(())) => {}
            Some(Err(e)) => nih_plug::nih_log!("Soba couldn't open the project's patch: {e}"),
            None => *shared.pending_state.lock() = Some(state),
        }
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&String) -> R,
    {
        let shared = self.shared.upgrade();
        let state = shared
            .as_ref()
            .and_then(|shared| shared.with_app(|app| app.hosted_state()).or_else(|| shared.pending_state.lock().clone()))
            .unwrap_or_default();
        f(&state)
    }
}
