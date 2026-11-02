//! Workspace collection identity and lookup helpers.
//!
//! The workspace stack continues to be driven by the layout's presentation engines. This module
//! keeps the stable collection identity separate from an output name so it survives hotplug.

use std::sync::atomic::{AtomicU64, Ordering};

use super::workspace::WorkspaceId;

static COLLECTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub(super) struct Collection {
    pub id: u64,
    pub name: Option<String>,
    /// Output whose collection this was originally, when applicable.
    pub original_output: Option<String>,
    /// Last active workspace, used when a collection is shown again.
    pub active_workspace: Option<WorkspaceId>,
    /// Whether this collection is declared in config and therefore persistent while empty.
    pub persistent: bool,
}

impl Collection {
    pub fn new(name: Option<String>, original_output: Option<String>, persistent: bool) -> Self {
        Self {
            id: COLLECTION_ID.fetch_add(1, Ordering::Relaxed),
            name,
            original_output,
            active_workspace: None,
            persistent,
        }
    }

    pub fn matches_reference(&self, reference: &niri_config::WorkspaceReference, idx: usize) -> bool {
        match reference {
            niri_config::WorkspaceReference::Id(id) => self.id == *id,
            niri_config::WorkspaceReference::Index(index) => usize::from(*index) == idx,
            niri_config::WorkspaceReference::Name(name) => {
                self.name.as_deref().is_some_and(|candidate| candidate == name)
            }
        }
    }
}
