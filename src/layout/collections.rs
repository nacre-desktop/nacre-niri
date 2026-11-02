//! Workspace collection identity and lookup helpers.
//!
//! The workspace stack continues to be driven by the layout's presentation engines. This module
//! keeps the stable collection identity separate from an output name so it survives hotplug.

use std::sync::atomic::{AtomicU64, Ordering};

use super::workspace::{OutputId, Workspace, WorkspaceId};
use super::LayoutElement;

static COLLECTION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(super) struct Collection<W: LayoutElement> {
    /// Stack owned here while the collection is not leased to a monitor.
    pub workspaces: Vec<Workspace<W>>,
    pub id: u64,
    pub name: Option<String>,
    /// Output whose collection this was originally, when applicable.
    pub original_output: Option<String>,
    home_output: Option<OutputId>,
    /// Last active workspace, used when a collection is shown again.
    pub active_workspace: Option<WorkspaceId>,
    /// Whether this collection is declared in config and therefore persistent while empty.
    pub persistent: bool,
}

impl<W: LayoutElement> Collection<W> {
    pub fn new(name: Option<String>, original_output: Option<String>, persistent: bool) -> Self {
        Self {
            workspaces: Vec::new(),
            id: COLLECTION_ID.fetch_add(1, Ordering::Relaxed),
            name,
            original_output,
            home_output: None,
            active_workspace: None,
            persistent,
        }
    }

    pub fn matches_reference(&self, reference: &niri_config::WorkspaceReference, idx: usize) -> bool {
        match reference {
            niri_config::WorkspaceReference::Id(id) => self.id == *id,
            niri_config::WorkspaceReference::Index(index) => *index != 0 && usize::from(*index) == idx + 1,
            niri_config::WorkspaceReference::Name(name) => {
                self.name.as_deref().is_some_and(|candidate| candidate == name)
            }
        }
    }
}


use niri_config::{LayoutPart, WorkspaceReference};
use smithay::output::Output;
use super::{ActivateWindow, Layout, MonitorSet};
use super::monitor::{Monitor, MonitorAddWindowTarget};
use super::workspace::WorkspaceAddWindowTarget;
use crate::utils::transaction::Transaction;

impl<W: LayoutElement> Layout<W> {
    pub fn collections(&self) -> Vec<niri_ipc::Collection> {
        if !self.options.layout.collections.enabled { return Vec::new(); }
        self.collections.iter().enumerate().map(|(idx, collection)| {
            let monitor = self.monitors().find(|m| m.collection_id == Some(collection.id));
            niri_ipc::Collection {
                id: collection.id, idx: idx + 1, name: collection.name.clone(),
                output: monitor.map(|m| m.output_name().clone()),
                is_active: monitor.is_some_and(|m| self.active_monitor_ref().is_some_and(|a| a.output() == m.output())),
                workspace_ids: self.collection_workspaces(collection.id).map(|ws| ws.id().get()).collect(),
            }
        }).collect()
    }

    pub(super) fn collection_registry(&self) -> &[Collection<W>] { &self.collections }

    pub fn collection_for_output(&self, output: &Output) -> Option<u64> {
        self.monitors().find(|m| m.output() == output).and_then(|m| m.collection_id)
    }

    pub fn collection_workspaces(&self, id: u64) -> impl Iterator<Item = &Workspace<W>> {
        let shown = self.monitors().filter(move |m| m.collection_id == Some(id)).flat_map(|m| m.workspaces.iter());
        let hidden = self.collections.iter().filter(move |c| c.id == id).flat_map(|c| c.workspaces.iter());
        shown.chain(hidden)
    }

    pub fn collection_active_workspace(&self, id: u64) -> Option<WorkspaceId> {
        self.monitors().find(|m| m.collection_id == Some(id)).map(|m| m.workspaces[m.active_workspace_idx].id())
            .or_else(|| self.collections.iter().find(|c| c.id == id).and_then(|c| c.active_workspace))
    }

    fn new_collection(&mut self, name: Option<String>, output: Option<String>, persistent: bool) -> u64 {
        let collection = Collection::new(name, output, persistent);
        let id = collection.id;
        self.collections.push(collection);
        id
    }

    pub fn create_collection(&mut self, name: Option<String>) -> u64 {
        if !self.options.layout.collections.enabled { return 0; }
        if let Some(existing) = name.as_ref().and_then(|name| self.collections.iter().find(|c| c.name.as_ref() == Some(name))) {
            let id = existing.id;
            self.focus_collection(&WorkspaceReference::Id(id));
            return id;
        }
        let id = self.new_collection(name, None, false);
        self.focus_collection(&WorkspaceReference::Id(id));
        id
    }

    /// Reconcile configured names without dropping stacks on config reload.
    pub(super) fn configure_collections(&mut self) {
        let names = self.options.layout.collections.names.clone();
        for c in &mut self.collections { c.persistent = c.name.as_ref().is_some_and(|name| names.contains(name)); }
        if self.options.layout.collections.enabled {
            for name in names {
                if !self.collections.iter().any(|c| c.name.as_ref() == Some(&name)) {
                    self.new_collection(Some(name), None, true);
                }
            }
        }
    }

    pub(super) fn transition_collections(&mut self, enabled: bool) {
        if enabled == self.options.layout.collections.enabled { return; }
        if enabled {
            let outputs: Vec<_> = self.monitors().map(|m| m.output.clone()).collect();
            for output in outputs {
                let id = self.new_collection(None, Some(output.name()), false);
                self.collections.iter_mut().find(|c| c.id == id).unwrap().home_output = Some(OutputId::new(&output));
                self.monitors_mut().find(|m| m.output() == &output).unwrap().collection_id = Some(id);
            }
        } else {
            let mut stacks: Vec<_> = self.collections.drain(..).flat_map(|c| c.workspaces).filter(|ws| ws.has_windows_or_name()).collect();
            match &mut self.monitor_set {
                MonitorSet::Normal { monitors, primary_idx, .. } => {
                    for mon in monitors.iter_mut() { mon.collection_id = None; }
                    monitors[*primary_idx].append_workspaces(stacks);
                }
                MonitorSet::NoOutputs { workspaces } => workspaces.append(&mut stacks),
            }
            self.collection_returns.clear();
        }
    }

    fn store_collection_stack(&mut self, id: u64, mut workspaces: Vec<Workspace<W>>, active: Option<WorkspaceId>) {
        for ws in &mut workspaces {
            ws.set_output(None);
            ws.update_config(self.options.clone());
            ws.refresh(false, false);
        }
        if let Some(c) = self.collections.iter_mut().find(|c| c.id == id) {
            c.workspaces = workspaces;
            c.active_workspace = active;
        }
    }

    fn take_collection_monitor(&mut self, id: u64, output: Output, layout: Option<LayoutPart>) -> Monitor<W> {
        let c = self.collections.iter_mut().find(|c| c.id == id).unwrap();
        let stack = std::mem::take(&mut c.workspaces);
        let active = c.active_workspace;
        let mut mon = Monitor::new(output.clone(), Vec::new(), None,
            self.clock.clone(), self.options.clone(), layout);
        if !stack.is_empty() {
            mon.workspaces = stack;
            mon.active_workspace_idx = active.and_then(|id| mon.workspaces.iter().position(|ws| ws.id() == id)).unwrap_or(0);
            for ws in &mut mon.workspaces {
                ws.set_output(Some(output.clone()));
                ws.update_config(mon.options.clone());
            }
            if mon.workspaces.last().is_some_and(|ws| ws.has_windows_or_name()) { mon.add_workspace_bottom(); }
            if mon.options.layout.empty_workspace_above_first && mon.workspaces.first().is_some_and(|ws| ws.has_windows_or_name()) { mon.add_workspace_top(); }
            mon.clean_up_workspaces();
        }
        mon.collection_id = Some(id);
        mon.overview_open = self.overview_open;
        mon.set_overview_progress(self.overview_progress.as_ref());
        mon
    }

    pub(super) fn add_collection_output(&mut self, output: Output, layout: Option<LayoutPart>) {
        self.configure_collections();
        let name = output.name();
        let id = self.collections.iter().find(|c| c.home_output.as_ref().is_some_and(|home| home.matches(&output)) || c.original_output.as_deref() == Some(&name)).map(|c| c.id)
            .unwrap_or_else(|| {
                // The first configured collection can serve as the first monitor's desk.
                if self.monitors().next().is_none() {
                    if let Some(c) = self.collections.iter_mut().find(|c| c.original_output.is_none() && c.persistent) {
                        c.original_output = Some(name.clone());
                        return c.id;
                    }
                }
                self.new_collection(None, Some(name.clone()), false)
            });
        let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
        collection.original_output = Some(name.clone());
        collection.home_output = Some(OutputId::new(&output));
        // A disconnected monitor's collection may currently be borrowed by another output.
        let borrower = self.monitors().find(|m| m.collection_id == Some(id)).map(|m| m.output_name().clone());
        if let Some(borrower) = borrower {
            let fallback = self.collection_returns.remove(&borrower).filter(|id| self.collections.iter().any(|c| c.id == *id))
                .unwrap_or_else(|| self.new_collection(None, Some(borrower.clone()), false));
            self.show_collection_on_output(&borrower, fallback);
        }
        let old = std::mem::take(&mut self.monitor_set);
        self.monitor_set = match old {
            MonitorSet::Normal { mut monitors, primary_idx, active_monitor_idx } => {
                monitors.push(self.take_collection_monitor(id, output, layout));
                MonitorSet::Normal { monitors, primary_idx, active_monitor_idx }
            }
            MonitorSet::NoOutputs { workspaces } => {
                // Workspaces configured before the first output belong to its initial desk.
                if !workspaces.is_empty() {
                    self.collections.iter_mut().find(|c| c.id == id).unwrap().workspaces.extend(workspaces);
                }
                let mon = self.take_collection_monitor(id, output, layout);
                MonitorSet::Normal { monitors: vec![mon], primary_idx: 0, active_monitor_idx: 0 }
            }
        };
    }

    pub(super) fn remove_collection_output(&mut self, output: &Output) {
        let MonitorSet::Normal { mut monitors, mut primary_idx, mut active_monitor_idx } = std::mem::take(&mut self.monitor_set) else { panic!("no outputs"); };
        let idx = monitors.iter().position(|m| m.output() == output).expect("unknown output");
        let mut mon = monitors.remove(idx);
        let id = mon.collection_id.unwrap();
        let active = mon.workspaces.get(mon.active_workspace_idx).map(|ws| ws.id());
        self.store_collection_stack(id, std::mem::take(&mut mon.workspaces), active);
        primary_idx = primary_idx.saturating_sub(usize::from(primary_idx >= idx));
        active_monitor_idx = active_monitor_idx.saturating_sub(usize::from(active_monitor_idx >= idx));
        self.monitor_set = if monitors.is_empty() { MonitorSet::NoOutputs { workspaces: vec![] } }
            else { MonitorSet::Normal { monitors, primary_idx, active_monitor_idx } };
        self.clean_up_collections();
    }

    fn show_collection_on_output(&mut self, output_name: &str, id: u64) {
        let Some((old_id, output, layout)) = self.monitors().find(|m| m.output_name() == output_name)
            .map(|m| (m.collection_id, m.output.clone(), m.layout_config.clone())) else { return; };
        if old_id == Some(id) { return; }
        let other = self.monitors().find(|m| m.collection_id == Some(id)).map(|m| m.output_name().clone());
        if let Some(other) = other {
            // Keep one collection per output: switching to an already shown desk swaps desks.
            let old_id = old_id.unwrap();
            let placeholder = self.new_collection(None, None, false);
            self.show_collection_on_output(&other, placeholder);
            self.show_collection_on_output(output_name, id);
            self.show_collection_on_output(&other, old_id);
            self.clean_up_collections();
            return;
        }
        let mut stack = Vec::new();
        let mut active = None;
        if let Some(mon) = self.monitors_mut().find(|m| m.output_name() == output_name) {
            active = mon.workspaces.get(mon.active_workspace_idx).map(|ws| ws.id());
            stack = std::mem::take(&mut mon.workspaces);
        }
        if let Some(old) = old_id { self.store_collection_stack(old, stack, active); }
        let new_monitor = self.take_collection_monitor(id, output, layout);
        if let Some(mon) = self.monitors_mut().find(|m| m.output_name() == output_name) { *mon = new_monitor; }
    }

    pub fn focus_collection(&mut self, reference: &WorkspaceReference) {
        if !self.options.layout.collections.enabled { return; }
        let Some(id) = self.collections.iter().enumerate().find(|(i,c)| c.matches_reference(reference,*i)).map(|(_,c)| c.id) else { return; };
        let Some((name, old)) = self.active_monitor_ref().map(|m| (m.output_name().clone(), m.collection_id)) else { return; };
        if self.collections.iter().any(|c| c.id == id && c.original_output.as_deref().is_some_and(|o| o != name)) {
            if let Some(old) = old { self.collection_returns.entry(name.clone()).or_insert(old); }
        }
        self.show_collection_on_output(&name, id);
        self.clean_up_collections();
    }

    fn adjacent_collection(&self, next: bool) -> Option<u64> {
        let current = self.active_monitor_ref()?.collection_id?;
        let idx = self.collections.iter().position(|c| c.id == current)?;
        let len = self.collections.len();
        Some(self.collections[(idx + if next { 1 } else { len - 1 }) % len].id)
    }
    pub fn focus_collection_next(&mut self) { if let Some(id) = self.adjacent_collection(true) { self.focus_collection(&WorkspaceReference::Id(id)); } }
    pub fn focus_collection_previous(&mut self) { if let Some(id) = self.adjacent_collection(false) { self.focus_collection(&WorkspaceReference::Id(id)); } }
    pub fn move_window_to_collection_next(&mut self) { if let Some(id) = self.adjacent_collection(true) { self.move_window_to_collection(&WorkspaceReference::Id(id)); } }
    pub fn move_window_to_collection_previous(&mut self) { if let Some(id) = self.adjacent_collection(false) { self.move_window_to_collection(&WorkspaceReference::Id(id)); } }

    pub fn move_window_to_collection(&mut self, reference: &WorkspaceReference) {
        if !self.options.layout.collections.enabled { return; }
        let Some(id) = self.collections.iter().enumerate().find(|(i,c)| c.matches_reference(reference,*i)).map(|(_,c)| c.id) else { return; };
        if self.active_monitor_ref().is_some_and(|m| m.collection_id == Some(id)) { return; }
        let Some(window) = self.active_workspace().and_then(|ws| ws.active_window()).map(|w| w.id().clone()) else { return; };
        let Some(removed) = self.remove_window(&window, Transaction::new()) else { return; };
        let shown = self.monitors().any(|m| m.collection_id == Some(id));
        if shown {
            let mon = self.monitors_mut().find(|m| m.collection_id == Some(id)).unwrap();
            mon.add_tile(removed.tile, MonitorAddWindowTarget::Auto, ActivateWindow::No, false, removed.width, removed.is_full_width, removed.is_floating);
        } else {
            let c = self.collections.iter_mut().find(|c| c.id == id).unwrap();
            if c.workspaces.is_empty() {
                c.workspaces.push(Workspace::new_with_config_no_outputs(None, self.clock.clone(), self.options.clone()));
            }
            let idx = c.active_workspace.and_then(|id| c.workspaces.iter().position(|ws| ws.id() == id)).unwrap_or(0);
            let ws = &mut c.workspaces[idx];
            ws.add_tile(removed.tile, WorkspaceAddWindowTarget::Auto, ActivateWindow::No, removed.width, removed.is_full_width, removed.is_floating);
            ws.refresh(false, false);
        }
        self.clean_up_collections();
    }

    pub(super) fn clean_up_collections(&mut self) {
        let shown: Vec<_> = self.monitors().filter_map(|m| m.collection_id).collect();
        let returns: Vec<_> = self.collection_returns.values().copied().collect();
        self.collections.retain(|c| c.persistent || shown.contains(&c.id) || returns.contains(&c.id) || c.workspaces.iter().any(|ws| ws.has_windows()));
    }
}
