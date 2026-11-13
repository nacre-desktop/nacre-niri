//! Versioned state and exact window routing rules for persistent named collections.
//!
//! The compositor calls `save_atomic` only after a debounced logical state change. Window routes
//! intentionally use stable app-id/title strings rather than runtime window IDs; those IDs change
//! between sessions and title matching is best effort for applications with dynamic titles.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::workspace::WorkspaceId;

const STATE_VERSION: u32 = 1;
const MAX_STATE_BYTES: usize = 4 * 1024 * 1024;
const MAX_COLLECTIONS: usize = 128;
const MAX_WORKSPACES_PER_COLLECTION: usize = 256;
const MAX_WINDOW_ROUTES: usize = 4096;
const MAX_COLLECTION_NAME_BYTES: usize = 256;
const MAX_WORKSPACE_NAME_BYTES: usize = 256;
const MAX_APP_ID_BYTES: usize = 1024;
const MAX_TITLE_BYTES: usize = 2048;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedCollections {
    pub version: u32,
    /// Ordered named collection stacks. Runtime IDs are deliberately not persisted.
    pub collections: Vec<PersistedCollection>,
    /// Historical routes stay here after their windows close or the user logs out.
    #[serde(default)]
    pub window_routes: Vec<PersistedWindowRoute>,
    /// Process-local IDs that tie persisted ordinals to their startup-created workspaces.
    #[serde(skip)]
    runtime_workspace_ids: HashMap<(String, usize), WorkspaceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedCollection {
    /// Runtime-facing name; it may differ from the original config declaration after rename.
    pub name: String,
    /// Config declaration that originally created this stack, retained as a stable alias.
    #[serde(default)]
    pub configured_origin: Option<String>,
    pub original_output: Option<String>,
    /// Vector order is workspace order, including unnamed workspaces.
    pub workspaces: Vec<PersistedWorkspace>,
    pub active_workspace: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWorkspace {
    pub name: Option<String>,
    /// Keep this unnamed slot when it is empty. Dynamic output placeholders leave this false.
    #[serde(default)]
    pub retain_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedWindowRoute {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub collection: String,
    pub workspace: PersistedWorkspaceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum PersistedWorkspaceRef {
    Name(String),
    Ordinal(usize),
}

impl PersistedCollection {
    fn fits_persistence_limits(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= MAX_COLLECTION_NAME_BYTES
            && self
                .configured_origin
                .as_ref()
                .is_none_or(|name| !name.is_empty() && name.len() <= MAX_COLLECTION_NAME_BYTES)
            && self.original_output.as_ref().is_none_or(|output| {
                !output.is_empty() && output.len() <= MAX_COLLECTION_NAME_BYTES
            })
            && !self.workspaces.is_empty()
            && self.workspaces.len() <= MAX_WORKSPACES_PER_COLLECTION
            && self.workspaces.iter().all(|workspace| {
                workspace
                    .name
                    .as_ref()
                    .is_none_or(|name| !name.is_empty() && name.len() <= MAX_WORKSPACE_NAME_BYTES)
            })
    }
}

impl PersistedWindowRoute {
    fn fits_persistence_limits(&self) -> bool {
        self.app_id
            .as_ref()
            .is_some_and(|id| !id.is_empty() && id.len() <= MAX_APP_ID_BYTES)
            && self
                .title
                .as_ref()
                .is_none_or(|title| title.len() <= MAX_TITLE_BYTES)
            && !self.collection.is_empty()
            && self.collection.len() <= MAX_COLLECTION_NAME_BYTES
            && match &self.workspace {
                PersistedWorkspaceRef::Name(name) => {
                    !name.is_empty() && name.len() <= MAX_WORKSPACE_NAME_BYTES
                }
                PersistedWorkspaceRef::Ordinal(index) => *index < MAX_WORKSPACES_PER_COLLECTION,
            }
    }
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    Io(io::Error),
    Invalid(&'static str),
    UnsupportedVersion(u32),
    TooLarge,
}

impl From<io::Error> for PersistenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "collection state I/O error: {error}"),
            Self::Invalid(reason) => write!(f, "invalid collection state: {reason}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported collection state version {version}")
            }
            Self::TooLarge => write!(f, "collection state exceeds {MAX_STATE_BYTES} bytes"),
        }
    }
}

impl std::error::Error for PersistenceError {}

impl PersistedCollections {
    pub fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            collections: Vec::new(),
            window_routes: Vec::new(),
            runtime_workspace_ids: HashMap::new(),
        }
    }

    pub fn same_persisted_state(&self, other: &Self) -> bool {
        self.version == other.version
            && self.collections == other.collections
            && self.window_routes == other.window_routes
    }

    pub fn validate(&self) -> Result<(), PersistenceError> {
        if self.version != STATE_VERSION {
            return Err(PersistenceError::UnsupportedVersion(self.version));
        }
        if self.collections.len() > MAX_COLLECTIONS {
            return Err(PersistenceError::Invalid("too many collections"));
        }
        if self.window_routes.len() > MAX_WINDOW_ROUTES {
            return Err(PersistenceError::Invalid("too many window routes"));
        }
        let mut workspace_names = std::collections::HashSet::new();
        for (idx, collection) in self.collections.iter().enumerate() {
            if collection.name.is_empty() || collection.name.len() > MAX_COLLECTION_NAME_BYTES {
                return Err(PersistenceError::Invalid("invalid collection name length"));
            }
            if collection
                .configured_origin
                .as_ref()
                .is_some_and(|name| name.is_empty() || name.len() > MAX_COLLECTION_NAME_BYTES)
            {
                return Err(PersistenceError::Invalid(
                    "invalid configured collection origin",
                ));
            }
            if collection
                .original_output
                .as_ref()
                .is_some_and(|output| output.is_empty() || output.len() > MAX_COLLECTION_NAME_BYTES)
            {
                return Err(PersistenceError::Invalid("invalid original output name"));
            }
            if self.collections[..idx]
                .iter()
                .any(|previous| previous.name == collection.name)
            {
                return Err(PersistenceError::Invalid("duplicate collection name"));
            }
            if collection.configured_origin.as_ref().is_some_and(|origin| {
                self.collections[..idx]
                    .iter()
                    .any(|previous| previous.configured_origin.as_ref() == Some(origin))
            }) {
                return Err(PersistenceError::Invalid(
                    "duplicate configured collection origin",
                ));
            }
            if collection.workspaces.is_empty()
                || collection.workspaces.len() > MAX_WORKSPACES_PER_COLLECTION
            {
                return Err(PersistenceError::Invalid("invalid workspace count"));
            }
            if collection.active_workspace >= collection.workspaces.len() {
                return Err(PersistenceError::Invalid(
                    "active workspace is out of range",
                ));
            }
            for workspace in &collection.workspaces {
                if workspace
                    .name
                    .as_ref()
                    .is_some_and(|name| name.is_empty() || name.len() > MAX_WORKSPACE_NAME_BYTES)
                {
                    return Err(PersistenceError::Invalid("invalid workspace name length"));
                }
                if let Some(name) = &workspace.name {
                    if !workspace_names.insert(name.to_ascii_lowercase()) {
                        return Err(PersistenceError::Invalid("duplicate workspace name"));
                    }
                }
            }
        }
        for route in &self.window_routes {
            if route
                .app_id
                .as_ref()
                .is_none_or(|id| id.is_empty() || id.len() > MAX_APP_ID_BYTES)
            {
                return Err(PersistenceError::Invalid("route needs a bounded app-id"));
            }
            if route
                .title
                .as_ref()
                .is_some_and(|title| title.len() > MAX_TITLE_BYTES)
            {
                return Err(PersistenceError::Invalid("window title is too long"));
            }
            if route.collection.is_empty() || route.collection.len() > MAX_COLLECTION_NAME_BYTES {
                return Err(PersistenceError::Invalid("invalid route collection name"));
            }
            match &route.workspace {
                PersistedWorkspaceRef::Name(name)
                    if name.is_empty() || name.len() > MAX_WORKSPACE_NAME_BYTES =>
                {
                    return Err(PersistenceError::Invalid("invalid route workspace name"));
                }
                PersistedWorkspaceRef::Ordinal(index)
                    if *index >= MAX_WORKSPACES_PER_COLLECTION =>
                {
                    return Err(PersistenceError::Invalid("route workspace is out of range"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Prefer an exact app-id/title rule. An app-id-only route is used only when the saved rule
    /// explicitly has no title. Runtime titles can change, so no prefix or fuzzy matching occurs.
    pub fn route_for_window(
        &self,
        app_id: Option<&str>,
        title: Option<&str>,
    ) -> Option<&PersistedWindowRoute> {
        let mut exact = self.window_routes.iter().filter(|route| {
            route
                .app_id
                .as_deref()
                .is_some_and(|saved| Some(saved) == app_id)
                && route
                    .title
                    .as_deref()
                    .is_some_and(|saved| title.is_some_and(|candidate| candidate == saved))
        });
        if let Some(first) = exact.next() {
            return exact
                .all(|route| {
                    route.collection == first.collection && route.workspace == first.workspace
                })
                .then_some(first);
        }
        if let Some(app_id) = app_id {
            let mut matching = self
                .window_routes
                .iter()
                .filter(|route| route.app_id.as_deref() == Some(app_id));
            if let Some(first) = matching.next() {
                let unambiguous = matching.all(|route| {
                    route.collection == first.collection && route.workspace == first.workspace
                });
                if unambiguous {
                    return Some(first);
                }
                return None;
            }
        }
        None
    }

    pub fn upsert_window_route(&mut self, route: PersistedWindowRoute) {
        if !route.fits_persistence_limits() {
            return;
        }
        if let Some(existing) = self
            .window_routes
            .iter_mut()
            .find(|existing| existing.app_id == route.app_id && existing.title == route.title)
        {
            *existing = route;
        } else if self.window_routes.len() < MAX_WINDOW_ROUTES {
            self.window_routes.push(route);
        }
    }

    /// Place configured collection names first in their configured order. Persisted names that
    /// are no longer in config retain their relative order after them.
    pub fn reconcile_configured_names(&mut self, configured: &[String]) {
        let saved = std::mem::take(&mut self.collections);
        let mut ordered = Vec::with_capacity(saved.len() + configured.len());
        let mut consumed = vec![false; saved.len()];
        for name in configured {
            if let Some(index) = saved.iter().enumerate().find_map(|(index, collection)| {
                (!consumed[index]
                    && (collection.configured_origin.as_deref() == Some(name)
                        || collection.name == *name))
                    .then_some(index)
            }) {
                consumed[index] = true;
                let mut collection = saved[index].clone();
                collection.configured_origin = Some(name.clone());
                ordered.push(collection);
            } else {
                ordered.push(PersistedCollection {
                    name: name.clone(),
                    configured_origin: Some(name.clone()),
                    original_output: None,
                    workspaces: vec![PersistedWorkspace {
                        name: None,
                        retain_empty: true,
                    }],
                    active_workspace: 0,
                });
            }
        }
        for (index, collection) in saved.into_iter().enumerate() {
            if consumed[index] {
                continue;
            }
            // An unchanged config-only collection stops persisting when its declaration is
            // removed. A runtime rename gives it an independent user-facing identity.
            if collection
                .configured_origin
                .as_ref()
                .is_some_and(|origin| collection.name == *origin && !configured.contains(origin))
            {
                continue;
            }
            ordered.push(collection);
        }
        let retained_names: std::collections::HashSet<_> = ordered
            .iter()
            .map(|collection| collection.name.as_str())
            .collect();
        self.window_routes
            .retain(|route| retained_names.contains(route.collection.as_str()));
        self.collections = ordered;
    }

    /// Refresh assignments for currently mapped windows while preserving routes for apps that
    /// are closed. Duplicate live app-id/title keys with different targets are omitted because
    /// those windows cannot be distinguished reliably after restart.
    pub fn replace_live_routes(&mut self, live_routes: Vec<PersistedWindowRoute>) {
        let mut groups: Vec<(Option<String>, Option<String>, Vec<PersistedWindowRoute>)> = vec![];
        for route in live_routes
            .into_iter()
            .filter(PersistedWindowRoute::fits_persistence_limits)
        {
            if let Some((_, _, routes)) = groups
                .iter_mut()
                .find(|(app_id, title, _)| *app_id == route.app_id && *title == route.title)
            {
                routes.push(route);
            } else {
                groups.push((route.app_id.clone(), route.title.clone(), vec![route]));
            }
        }
        self.window_routes.retain(|saved| {
            !groups
                .iter()
                .any(|(app_id, title, _)| saved.app_id == *app_id && saved.title == *title)
        });
        for (_, _, routes) in groups {
            let Some(first) = routes.first() else {
                continue;
            };
            if routes.iter().all(|route| {
                route.collection == first.collection && route.workspace == first.workspace
            }) {
                self.upsert_window_route(first.clone());
            }
        }
    }

    /// Drop unsupported stacks and route keys, then keep a deterministic prefix that fits the
    /// versioned file-size bound. Live layout names are not changed by this persistence filter.
    pub(crate) fn fit_for_persistence(&mut self) {
        let mut collections = std::mem::take(&mut self.collections);
        collections.retain(PersistedCollection::fits_persistence_limits);
        collections.truncate(MAX_COLLECTIONS);
        let mut routes = std::mem::take(&mut self.window_routes);
        routes.retain(PersistedWindowRoute::fits_persistence_limits);
        routes.truncate(MAX_WINDOW_ROUTES);

        let mut fitted = Self {
            version: self.version,
            collections: Vec::with_capacity(collections.len()),
            window_routes: Vec::with_capacity(routes.len()),
            runtime_workspace_ids: std::mem::take(&mut self.runtime_workspace_ids),
        };
        let mut serialized_size =
            serde_json::to_vec(&fitted).map_or(usize::MAX, |bytes| bytes.len());
        for collection in collections {
            let entry_size =
                serde_json::to_vec(&collection).map_or(usize::MAX, |bytes| bytes.len());
            let additional = entry_size.saturating_add(usize::from(!fitted.collections.is_empty()));
            if serialized_size.saturating_add(additional) <= MAX_STATE_BYTES {
                serialized_size += additional;
                fitted.collections.push(collection);
            }
        }

        let collection_indexes: HashMap<_, _> = fitted
            .collections
            .iter()
            .map(|collection| (collection.name.as_str(), collection))
            .collect();
        fitted
            .runtime_workspace_ids
            .retain(|(name, _), _| collection_indexes.contains_key(name.as_str()));

        for route in routes {
            let Some(collection) = collection_indexes.get(route.collection.as_str()) else {
                continue;
            };
            let workspace_exists = match &route.workspace {
                PersistedWorkspaceRef::Name(name) => collection
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.name.as_deref() == Some(name)),
                PersistedWorkspaceRef::Ordinal(index) => *index < collection.workspaces.len(),
            };
            if !workspace_exists {
                continue;
            }
            let entry_size = serde_json::to_vec(&route).map_or(usize::MAX, |bytes| bytes.len());
            let additional =
                entry_size.saturating_add(usize::from(!fitted.window_routes.is_empty()));
            if serialized_size.saturating_add(additional) <= MAX_STATE_BYTES {
                serialized_size += additional;
                fitted.window_routes.push(route);
            }
        }
        *self = fitted;
    }

    /// Closed-window rules keep their unnamed target slot alive across the next restart.
    pub fn retain_routed_empty_workspaces(&mut self) {
        for route in &self.window_routes {
            let Some(collection) = self
                .collections
                .iter_mut()
                .find(|collection| collection.name == route.collection)
            else {
                continue;
            };
            let index = match &route.workspace {
                PersistedWorkspaceRef::Name(name) => collection
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.name.as_deref() == Some(name)),
                PersistedWorkspaceRef::Ordinal(index) => Some(*index),
            };
            if let Some(workspace) = index.and_then(|index| collection.workspaces.get_mut(index)) {
                workspace.retain_empty = true;
            }
        }
    }

    /// Migrate old routes through stable in-process workspace IDs after collection or workspace
    /// rename/reorder. Any route whose old target no longer exists is discarded.
    pub fn migrate_routes_to_runtime_locations(
        &mut self,
        previous: &PersistedCollections,
        current: &HashMap<WorkspaceId, (String, PersistedWorkspaceRef)>,
    ) {
        self.window_routes.retain_mut(|route| {
            let Some(old_collection) = previous
                .collections
                .iter()
                .find(|collection| collection.name == route.collection)
            else {
                return false;
            };
            let old_ordinal = match &route.workspace {
                PersistedWorkspaceRef::Name(name) => old_collection
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.name.as_deref() == Some(name)),
                PersistedWorkspaceRef::Ordinal(index) => Some(*index),
            };
            let Some(workspace_id) = old_ordinal.and_then(|index| {
                previous
                    .runtime_workspace_ids
                    .get(&(route.collection.clone(), index))
                    .copied()
            }) else {
                return false;
            };
            let Some((collection, workspace)) = current.get(&workspace_id) else {
                return false;
            };
            route.collection = collection.clone();
            route.workspace = workspace.clone();
            true
        });
        self.runtime_workspace_ids.clear();
        for (workspace_id, (collection, workspace)) in current {
            let Some(persisted_collection) = self
                .collections
                .iter()
                .find(|candidate| candidate.name == *collection)
            else {
                continue;
            };
            let index = match workspace {
                PersistedWorkspaceRef::Name(name) => persisted_collection
                    .workspaces
                    .iter()
                    .position(|saved| saved.name.as_deref() == Some(name)),
                PersistedWorkspaceRef::Ordinal(index) => Some(*index),
            };
            if let Some(index) = index {
                self.runtime_workspace_ids
                    .insert((collection.clone(), index), *workspace_id);
            }
        }
    }

    /// Keep historical window assignments attached when a collection is renamed.
    #[cfg(test)]
    pub fn rename_collection(&mut self, previous: &str, next: &str) -> bool {
        if previous == next {
            return self
                .collections
                .iter()
                .any(|collection| collection.name == previous);
        }
        if next.is_empty()
            || next.len() > MAX_COLLECTION_NAME_BYTES
            || self
                .collections
                .iter()
                .any(|collection| collection.name == next)
        {
            return false;
        }
        let Some(collection) = self.collections.iter_mut().find(|c| c.name == previous) else {
            return false;
        };
        collection.name = next.to_owned();
        for route in &mut self.window_routes {
            if route.collection == previous {
                route.collection = next.to_owned();
            }
        }
        true
    }

    /// Migrate a named workspace route before changing its name. When the workspace becomes
    /// unnamed, preserve assignments by its collection-local ordinal instead.
    #[cfg(test)]
    pub fn rename_workspace(
        &mut self,
        collection_name: &str,
        previous: &str,
        next: Option<&str>,
    ) -> bool {
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|c| c.name == collection_name)
        else {
            return false;
        };
        if next == Some(previous) {
            return collection
                .workspaces
                .iter()
                .any(|workspace| workspace.name.as_deref() == Some(previous));
        }
        if next.is_some_and(|name| {
            name.is_empty()
                || name.len() > MAX_WORKSPACE_NAME_BYTES
                || collection
                    .workspaces
                    .iter()
                    .any(|ws| ws.name.as_deref() == Some(name))
        }) {
            return false;
        }
        let Some((index, workspace)) = collection
            .workspaces
            .iter_mut()
            .enumerate()
            .find(|(_, workspace)| workspace.name.as_deref() == Some(previous))
        else {
            return false;
        };
        workspace.name = next.map(str::to_owned);
        for route in &mut self.window_routes {
            if route.collection == collection_name
                && route.workspace == PersistedWorkspaceRef::Name(previous.to_owned())
            {
                route.workspace = next.map_or(PersistedWorkspaceRef::Ordinal(index), |name| {
                    PersistedWorkspaceRef::Name(name.to_owned())
                });
            }
        }
        true
    }

    pub fn load() -> Result<Option<Self>, PersistenceError> {
        let Some(path) = state_path() else {
            return Ok(None);
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Option<Self>, PersistenceError> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.len() > MAX_STATE_BYTES as u64 {
            return Err(PersistenceError::TooLarge);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take((MAX_STATE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(PersistenceError::TooLarge);
        }
        let state: Self = serde_json::from_slice(&bytes)
            .map_err(|_| PersistenceError::Invalid("state is not valid versioned JSON"))?;
        state.validate()?;
        Ok(Some(state))
    }

    pub fn save_atomic(&self) -> Result<(), PersistenceError> {
        let path = state_path().ok_or(PersistenceError::Invalid(
            "XDG_STATE_HOME and HOME do not resolve to an absolute path",
        ))?;
        self.save_atomic_to(&path)
    }

    pub fn save_atomic_to(&self, path: &Path) -> Result<(), PersistenceError> {
        self.validate()?;
        // Preserve a future schema version and corrupt state for recovery instead of silently
        // replacing a file this build cannot interpret.
        if path.exists() {
            let _ = Self::load_from(path)?;
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|_| PersistenceError::Invalid("state could not be serialized"))?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(PersistenceError::TooLarge);
        }
        let parent = path.parent().ok_or(PersistenceError::Invalid(
            "state path has no parent directory",
        ))?;
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let (temp_path, mut file) = create_temp(path)?;
        let result = (|| -> Result<(), PersistenceError> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp_path, path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp_path);
        }
        result
    }
}

impl<W: super::LayoutElement> super::Layout<W> {
    /// Restore saved named stacks before outputs are added. Existing config workspaces are moved
    /// into their saved stack by name, while unnamed saved slots get fresh runtime IDs.
    pub(crate) fn restore_persisted_collections(
        &mut self,
        state: &mut PersistedCollections,
    ) -> bool {
        if !self.options.layout.collections.enabled
            || self.monitors().next().is_some()
            || state.validate().is_err()
        {
            return false;
        }
        let mut prepared = state.clone();
        prepared.reconcile_configured_names(&self.options.layout.collections.names);
        prepared.runtime_workspace_ids.clear();
        for saved in &prepared.collections {
            let collection_id = self
                .collections
                .iter()
                .find(|collection| {
                    collection.name.as_deref() == Some(saved.name.as_str())
                        || saved.configured_origin.as_ref().is_some_and(|origin| {
                            collection.configured_name.as_ref() == Some(origin)
                        })
                        || collection.configured_name.as_ref() == Some(&saved.name)
                })
                .map(|collection| collection.id)
                .unwrap_or_else(|| self.create_collection(Some(saved.name.clone())));
            let mut restored = Vec::with_capacity(saved.workspaces.len());
            for (ordinal, workspace) in saved.workspaces.iter().enumerate() {
                let restored_workspace = if let Some(name) = &workspace.name {
                    self.take_workspace_by_name(name).unwrap_or_else(|| {
                        let config = niri_config::Workspace {
                            name: niri_config::workspace::WorkspaceName(name.clone()),
                            open_on_output: None,
                            layout: None,
                        };
                        super::workspace::Workspace::new_with_config_no_outputs(
                            Some(config),
                            self.clock.clone(),
                            self.options.clone(),
                        )
                    })
                } else {
                    super::workspace::Workspace::new_no_outputs(
                        self.clock.clone(),
                        self.options.clone(),
                    )
                };
                let mut restored_workspace = restored_workspace;
                self.collection_pending_workspaces
                    .retain(|pending| *pending != restored_workspace.id());
                restored_workspace.set_collection_persistent(
                    workspace.retain_empty || ordinal == saved.active_workspace,
                );
                restored.push(restored_workspace);
            }
            for (ordinal, workspace) in restored.iter().enumerate() {
                prepared
                    .runtime_workspace_ids
                    .insert((saved.name.clone(), ordinal), workspace.id());
            }
            if let Some(collection) = self.collections.iter_mut().find(|c| c.id == collection_id) {
                collection.name = Some(saved.name.clone());
                collection.configured_name = saved.configured_origin.clone();
                collection.user_named = saved
                    .configured_origin
                    .as_ref()
                    .is_none_or(|origin| origin != &saved.name);
                collection.persistent = collection.user_named
                    || self.options.layout.collections.names.contains(&saved.name)
                    || saved.configured_origin.as_ref().is_some_and(|origin| {
                        self.options.layout.collections.names.contains(origin)
                    });
                if saved.original_output.is_some() {
                    collection.original_output = saved.original_output.clone();
                }
                collection.workspaces = restored;
                collection.active_workspace = collection
                    .workspaces
                    .get(saved.active_workspace)
                    .map(|workspace| workspace.id());
            }
        }
        let order: HashMap<_, _> = prepared
            .collections
            .iter()
            .enumerate()
            .map(|(index, collection)| (collection.name.clone(), index))
            .collect();
        self.collections.sort_by_key(|collection| {
            order
                .get(&collection.name.clone().unwrap_or_default())
                .copied()
                .unwrap_or(usize::MAX)
        });
        *state = prepared;
        true
    }

    fn take_workspace_by_name(&mut self, name: &str) -> Option<super::workspace::Workspace<W>> {
        if let super::MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
            if let Some(index) = workspaces.iter().position(|workspace| {
                workspace
                    .name()
                    .is_some_and(|saved| saved.eq_ignore_ascii_case(name))
            }) {
                return Some(workspaces.remove(index));
            }
        }
        for collection in &mut self.collections {
            if let Some(index) = collection.workspaces.iter().position(|workspace| {
                workspace
                    .name()
                    .is_some_and(|saved| saved.eq_ignore_ascii_case(name))
            }) {
                return Some(collection.workspaces.remove(index));
            }
        }
        None
    }

    /// Cheap change detector for the persisted inputs, run on every refresh: workspace
    /// revisions cover window membership and toplevel commits (app ids, titles).
    pub(crate) fn persisted_fingerprint(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        for collection in self.collections.iter().filter(|c| c.persistent) {
            (
                collection.id,
                &collection.name,
                &collection.configured_name,
                &collection.original_output,
                self.collection_active_workspace(collection.id),
            )
                .hash(&mut hasher);
            for workspace in self.collection_workspaces(collection.id) {
                (
                    workspace.id(),
                    workspace.name(),
                    workspace.overview_revision(),
                    workspace.collection_persistent(),
                )
                    .hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Snapshot only persistent named collections; workspace vector order records unnamed slots.
    pub(crate) fn persisted_collection_snapshot(&self) -> Vec<PersistedCollection> {
        self.collections
            .iter()
            .filter(|collection| collection.persistent)
            .filter_map(|collection| {
                let name = collection.name.clone()?;
                let active_id = self.collection_active_workspace(collection.id);
                let workspaces_with_ids: Vec<_> = self
                    .collection_workspaces(collection.id)
                    .filter(|workspace| {
                        workspace.has_windows_or_name() || Some(workspace.id()) == active_id
                    })
                    .map(|workspace| {
                        (
                            workspace.id(),
                            PersistedWorkspace {
                                name: workspace.name().cloned(),
                                retain_empty: workspace.collection_persistent()
                                    || workspace.has_windows()
                                    || Some(workspace.id()) == active_id,
                            },
                        )
                    })
                    .collect();
                let workspaces: Vec<_> = workspaces_with_ids
                    .iter()
                    .map(|(_, workspace)| workspace.clone())
                    .collect();
                if workspaces.is_empty() {
                    return None;
                }
                let active_workspace = active_id
                    .and_then(|active| workspaces_with_ids.iter().position(|(id, _)| *id == active))
                    .unwrap_or(0);
                Some(PersistedCollection {
                    name,
                    configured_origin: collection.configured_name.clone(),
                    original_output: collection.original_output.clone(),
                    workspaces,
                    active_workspace,
                })
            })
            .collect()
    }

    pub(crate) fn persisted_workspace_id(
        &self,
        state: &PersistedCollections,
        collection_name: &str,
        workspace: &PersistedWorkspaceRef,
    ) -> Option<WorkspaceId> {
        let collection = self
            .collections
            .iter()
            .find(|collection| collection.name.as_deref() == Some(collection_name))?;
        if let PersistedWorkspaceRef::Ordinal(index) = workspace {
            if let Some(id) = state
                .runtime_workspace_ids
                .get(&(collection_name.to_owned(), *index))
            {
                return self
                    .collection_workspaces(collection.id)
                    .any(|workspace| workspace.id() == *id)
                    .then_some(*id);
            }
        }
        let active_id = self.collection_active_workspace(collection.id);
        self.collection_workspaces(collection.id)
            .filter(|workspace| {
                workspace.has_windows_or_name() || Some(workspace.id()) == active_id
            })
            .enumerate()
            .find(|(index, item)| match workspace {
                PersistedWorkspaceRef::Name(name) => item.name().is_some_and(|n| n == name),
                PersistedWorkspaceRef::Ordinal(ordinal) => index == ordinal,
            })
            .map(|(_, workspace)| workspace.id())
    }

    #[cfg(test)]
    pub(crate) fn persisted_workspace_location(
        &self,
        workspace_id: super::workspace::WorkspaceId,
    ) -> Option<(String, PersistedWorkspaceRef)> {
        for collection in &self.collections {
            let Some(name) = &collection.name else {
                continue;
            };
            if !collection.persistent {
                continue;
            }
            let active_id = self.collection_active_workspace(collection.id);
            if let Some((index, workspace)) = self
                .collection_workspaces(collection.id)
                .filter(|workspace| {
                    workspace.has_windows_or_name() || Some(workspace.id()) == active_id
                })
                .enumerate()
                .find(|(_, workspace)| workspace.id() == workspace_id)
            {
                let reference = workspace.name().map_or_else(
                    || PersistedWorkspaceRef::Ordinal(index),
                    |name| PersistedWorkspaceRef::Name(name.clone()),
                );
                return Some((name.clone(), reference));
            }
        }
        None
    }

    pub(crate) fn persisted_workspace_locations(
        &self,
    ) -> HashMap<WorkspaceId, (String, PersistedWorkspaceRef)> {
        let mut locations = HashMap::new();
        for collection in self
            .collections
            .iter()
            .filter(|collection| collection.persistent)
        {
            let Some(name) = &collection.name else {
                continue;
            };
            let active_id = self.collection_active_workspace(collection.id);
            for (ordinal, workspace) in self
                .collection_workspaces(collection.id)
                .filter(|workspace| {
                    workspace.has_windows_or_name() || Some(workspace.id()) == active_id
                })
                .enumerate()
            {
                let reference = workspace.name().map_or_else(
                    || PersistedWorkspaceRef::Ordinal(ordinal),
                    |workspace_name| PersistedWorkspaceRef::Name(workspace_name.clone()),
                );
                locations.insert(workspace.id(), (name.clone(), reference));
            }
        }
        locations
    }

    /// Refresh process-local ordinal IDs after a layout snapshot has changed. Output-local empty
    /// placeholders are excluded exactly as they are from the persisted stack.
    #[cfg(test)]
    pub(crate) fn refresh_persisted_workspace_ids(&self, state: &mut PersistedCollections) {
        state.runtime_workspace_ids.clear();
        for collection in self.collections.iter().filter(|c| c.persistent) {
            let Some(name) = &collection.name else {
                continue;
            };
            let active_id = self.collection_active_workspace(collection.id);
            for (ordinal, workspace) in self
                .collection_workspaces(collection.id)
                .filter(|workspace| {
                    workspace.has_windows_or_name() || Some(workspace.id()) == active_id
                })
                .enumerate()
            {
                state
                    .runtime_workspace_ids
                    .insert((name.clone(), ordinal), workspace.id());
            }
        }
    }

    /// Once a slot has been committed to disk, retain it after its last window closes so the
    /// historical route and ordinal still refer to the same workspace.
    pub(crate) fn retain_persisted_workspace_slots(&mut self, state: &PersistedCollections) {
        let workspace_ids: Vec<_> = state
            .collections
            .iter()
            .flat_map(|collection| {
                collection
                    .workspaces
                    .iter()
                    .enumerate()
                    .filter(|(_, workspace)| workspace.retain_empty)
                    .filter_map(|(ordinal, _)| {
                        state
                            .runtime_workspace_ids
                            .get(&(collection.name.clone(), ordinal))
                            .copied()
                    })
            })
            .collect();
        for id in workspace_ids {
            for collection in &mut self.collections {
                if let Some(workspace) = collection.workspaces.iter_mut().find(|ws| ws.id() == id) {
                    workspace.set_collection_persistent(true);
                }
            }
            match &mut self.monitor_set {
                super::MonitorSet::Normal { monitors, .. } => {
                    for monitor in monitors {
                        if let Some(workspace) =
                            monitor.workspaces.iter_mut().find(|ws| ws.id() == id)
                        {
                            workspace.set_collection_persistent(true);
                        }
                    }
                }
                super::MonitorSet::NoOutputs { workspaces } => {
                    if let Some(workspace) = workspaces.iter_mut().find(|ws| ws.id() == id) {
                        workspace.set_collection_persistent(true);
                    }
                }
            }
        }
    }
}

fn state_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })?;
    root.is_absolute()
        .then(|| root.join("nacre-niri/collections.json"))
}

fn temporary_path(path: &Path) -> PathBuf {
    let counter = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("collections.json");
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

fn create_temp(path: &Path) -> Result<(PathBuf, File), PersistenceError> {
    for _ in 0..16 {
        let candidate = temporary_path(path);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique collection-state temporary file",
    )
    .into())
}

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> io::Result<TestDirectory> {
        let path = std::env::temp_dir().join(format!(
            "nacre-niri-persistence-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(TestDirectory(path))
    }

    fn sample_state() -> PersistedCollections {
        PersistedCollections {
            version: STATE_VERSION,
            collections: vec![PersistedCollection {
                name: "work".into(),
                configured_origin: Some("work".into()),
                original_output: Some("DP-1".into()),
                workspaces: vec![
                    PersistedWorkspace {
                        name: Some("editor".into()),
                        retain_empty: false,
                    },
                    PersistedWorkspace {
                        name: None,
                        retain_empty: true,
                    },
                ],
                active_workspace: 1,
            }],
            window_routes: vec![PersistedWindowRoute {
                app_id: Some("org.example.Editor".into()),
                title: Some("Review".into()),
                collection: "work".into(),
                workspace: PersistedWorkspaceRef::Name("editor".into()),
            }],
            runtime_workspace_ids: HashMap::new(),
        }
    }

    #[test]
    fn versioned_state_round_trips_order_names_and_active_workspace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("collections.json");
        let mut state = sample_state();
        state
            .runtime_workspace_ids
            .insert(("work".into(), 1), WorkspaceId::specific(77));
        state.save_atomic_to(&path).unwrap();
        let loaded = PersistedCollections::load_from(&path).unwrap().unwrap();
        assert!(loaded.runtime_workspace_ids.is_empty());
        state.runtime_workspace_ids.clear();
        assert_eq!(Some(loaded), Some(state));
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("runtime_workspace_ids"));
        assert!(!saved.contains("77"));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn oversized_live_rules_and_stacks_do_not_block_other_desk_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("collections.json");
        let mut state = sample_state();
        let original_routes = state.window_routes.clone();

        state.upsert_window_route(PersistedWindowRoute {
            app_id: Some("a".repeat(MAX_APP_ID_BYTES + 1)),
            title: None,
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Name("editor".into()),
        });
        state.upsert_window_route(PersistedWindowRoute {
            app_id: Some("org.example.Editor".into()),
            title: Some("t".repeat(MAX_TITLE_BYTES + 1)),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Name("editor".into()),
        });
        state.replace_live_routes(vec![PersistedWindowRoute {
            app_id: Some("org.example.Editor".into()),
            title: Some("t".repeat(MAX_TITLE_BYTES + 1)),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Name("editor".into()),
        }]);
        assert_eq!(state.window_routes, original_routes);

        state.collections.push(PersistedCollection {
            name: "d".repeat(MAX_COLLECTION_NAME_BYTES + 1),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: None,
                retain_empty: true,
            }],
            active_workspace: 0,
        });
        state.collections.push(PersistedCollection {
            name: "workspace-limit".into(),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: Some("w".repeat(MAX_WORKSPACE_NAME_BYTES + 1)),
                retain_empty: true,
            }],
            active_workspace: 0,
        });
        state.window_routes.extend([
            PersistedWindowRoute {
                app_id: Some("org.example.LongDesk".into()),
                title: None,
                collection: "d".repeat(MAX_COLLECTION_NAME_BYTES + 1),
                workspace: PersistedWorkspaceRef::Ordinal(0),
            },
            PersistedWindowRoute {
                app_id: Some("org.example.LongWorkspace".into()),
                title: None,
                collection: "workspace-limit".into(),
                workspace: PersistedWorkspaceRef::Ordinal(0),
            },
        ]);

        state.fit_for_persistence();
        assert_eq!(
            state
                .collections
                .iter()
                .map(|collection| collection.name.as_str())
                .collect::<Vec<_>>(),
            ["work"]
        );
        assert_eq!(state.window_routes, original_routes);
        assert!(state.validate().is_ok());
        state.save_atomic_to(&path).unwrap();
        assert_eq!(PersistedCollections::load_from(&path).unwrap(), Some(state));
    }

    #[test]
    fn fitting_many_valid_stacks_keeps_the_serialized_state_under_the_file_limit() {
        let mut state = PersistedCollections::empty();
        let suffix = "x".repeat(220);
        for collection_idx in 0..100 {
            let workspaces = (0..MAX_WORKSPACES_PER_COLLECTION)
                .map(|workspace_idx| PersistedWorkspace {
                    name: Some(format!("desk{collection_idx}-ws{workspace_idx}-{suffix}")),
                    retain_empty: false,
                })
                .collect();
            state.collections.push(PersistedCollection {
                name: format!("desk{collection_idx}"),
                configured_origin: None,
                original_output: None,
                workspaces,
                active_workspace: 0,
            });
        }

        state.fit_for_persistence();
        assert!(state.collections.len() < 100);
        assert!(state.validate().is_ok());
        assert!(serde_json::to_vec(&state).unwrap().len() <= MAX_STATE_BYTES);
    }

    #[test]
    fn exact_title_route_precedes_fallback_and_refuses_ambiguity() {
        let mut state = sample_state();
        state.window_routes.push(PersistedWindowRoute {
            app_id: Some("org.example.Editor".into()),
            title: None,
            collection: "personal".into(),
            workspace: PersistedWorkspaceRef::Ordinal(0),
        });
        assert_eq!(
            state
                .route_for_window(Some("org.example.Editor"), Some("Review"))
                .unwrap()
                .collection,
            "work"
        );
        assert!(state
            .route_for_window(Some("org.example.Editor"), Some("Welcome"))
            .is_none());
        assert!(state
            .route_for_window(Some("org.example.Editor"), None)
            .is_none());
        assert!(state.route_for_window(None, Some("Review")).is_none());
    }

    #[test]
    fn app_id_fallback_requires_every_saved_title_to_share_one_target() {
        let mut state = sample_state();
        state.window_routes.push(PersistedWindowRoute {
            app_id: Some("org.example.Editor".into()),
            title: Some("Planning".into()),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Name("editor".into()),
        });
        assert_eq!(
            state
                .route_for_window(Some("org.example.Editor"), Some("Welcome"))
                .unwrap()
                .collection,
            "work"
        );
        state.window_routes[1].collection = "personal".into();
        assert!(state
            .route_for_window(Some("org.example.Editor"), Some("Welcome"))
            .is_none());
    }

    #[test]
    fn live_route_refresh_keeps_closed_rules_and_drops_conflicting_duplicates() {
        let mut state = sample_state();
        state.upsert_window_route(PersistedWindowRoute {
            app_id: Some("org.example.Closed".into()),
            title: Some("Closed".into()),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Ordinal(1),
        });
        state.replace_live_routes(vec![
            PersistedWindowRoute {
                app_id: Some("org.example.Editor".into()),
                title: Some("Review".into()),
                collection: "work".into(),
                workspace: PersistedWorkspaceRef::Name("editor".into()),
            },
            PersistedWindowRoute {
                app_id: Some("org.example.Editor".into()),
                title: Some("Review".into()),
                collection: "personal".into(),
                workspace: PersistedWorkspaceRef::Ordinal(0),
            },
        ]);
        assert!(state
            .route_for_window(Some("org.example.Editor"), Some("Review"))
            .is_none());
        assert!(state
            .route_for_window(Some("org.example.Closed"), Some("Closed"))
            .is_some());
    }

    #[test]
    fn collection_and_workspace_renames_migrate_closed_window_routes() {
        let mut state = sample_state();
        assert!(state.rename_collection("work", "studio"));
        assert!(state.rename_workspace("studio", "editor", Some("code")));
        let route = &state.window_routes[0];
        assert_eq!(route.collection, "studio");
        assert_eq!(route.workspace, PersistedWorkspaceRef::Name("code".into()));
        assert_eq!(
            state.route_for_window(Some("org.example.Editor"), Some("Review")),
            Some(route)
        );
    }

    #[test]
    fn configured_origin_coalesces_a_runtime_renamed_collection_without_resetting_name() {
        let mut state = sample_state();
        assert!(state.rename_collection("work", "research"));
        state.reconcile_configured_names(&["work".into()]);
        assert_eq!(state.collections.len(), 1);
        assert_eq!(state.collections[0].name, "research");
        assert_eq!(
            state.collections[0].configured_origin.as_deref(),
            Some("work")
        );
    }

    #[test]
    fn removing_config_drops_unchanged_origin_but_keeps_a_runtime_renamed_desk() {
        let mut unchanged = sample_state();
        unchanged.reconcile_configured_names(&[]);
        assert!(unchanged.collections.is_empty());
        assert!(unchanged.window_routes.is_empty());

        let mut renamed = sample_state();
        assert!(renamed.rename_collection("work", "research"));
        renamed.reconcile_configured_names(&[]);
        assert_eq!(renamed.collections.len(), 1);
        assert_eq!(renamed.collections[0].name, "research");
        assert_eq!(renamed.window_routes[0].collection, "research");
    }

    #[test]
    fn oversized_invalid_and_unknown_version_state_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("collections.json");
        fs::write(&path, vec![b' '; MAX_STATE_BYTES + 1]).unwrap();
        assert!(matches!(
            PersistedCollections::load_from(&path),
            Err(PersistenceError::TooLarge)
        ));
        let mut state = sample_state();
        state.version += 1;
        assert!(matches!(
            state.validate(),
            Err(PersistenceError::UnsupportedVersion(_))
        ));
        let mut bad = sample_state();
        bad.collections[0].active_workspace = usize::MAX;
        assert!(matches!(bad.validate(), Err(PersistenceError::Invalid(_))));

        let mut duplicate_workspace = sample_state();
        duplicate_workspace.collections.push(PersistedCollection {
            name: "personal".into(),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: Some("EDITOR".into()),
                retain_empty: false,
            }],
            active_workspace: 0,
        });
        assert!(matches!(
            duplicate_workspace.validate(),
            Err(PersistenceError::Invalid("duplicate workspace name"))
        ));

        fs::write(
            &path,
            br#"{"version":99,"collections":[],"window_routes":[]}"#,
        )
        .unwrap();
        let valid = sample_state();
        assert!(matches!(
            valid.save_atomic_to(&path),
            Err(PersistenceError::UnsupportedVersion(99))
        ));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()
                ["version"],
            99
        );
    }
}
