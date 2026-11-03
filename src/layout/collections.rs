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
    /// Whether persistence comes from a runtime-created or renamed desk.
    pub user_named: bool,
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
            user_named: false,
        }
    }

    pub fn matches_reference(
        &self,
        reference: &niri_config::WorkspaceReference,
        idx: usize,
    ) -> bool {
        match reference {
            niri_config::WorkspaceReference::Id(id) => self.id == *id,
            niri_config::WorkspaceReference::Index(index) => {
                *index != 0 && usize::from(*index) == idx + 1
            }
            niri_config::WorkspaceReference::Name(name) => self
                .name
                .as_deref()
                .is_some_and(|candidate| candidate == name),
        }
    }
}

use niri_config::{LayoutPart, WorkspaceReference};
use smithay::output::Output;

use super::monitor::{Monitor, MonitorAddWindowTarget};
use super::workspace::WorkspaceAddWindowTarget;
use super::{output_size, ActivateWindow, Layout, MonitorSet};
use crate::utils::transaction::Transaction;

impl<W: LayoutElement> Layout<W> {
    pub fn collections(&self) -> Vec<niri_ipc::Collection> {
        if !self.options.layout.collections.enabled {
            return Vec::new();
        }
        self.collections
            .iter()
            .enumerate()
            .map(|(idx, collection)| {
                let monitor = self
                    .monitors()
                    .find(|m| m.collection_id == Some(collection.id));
                niri_ipc::Collection {
                    id: collection.id,
                    idx: idx + 1,
                    name: collection.name.clone(),
                    output: monitor.map(|m| m.output_name().clone()),
                    is_active: monitor.is_some_and(|m| {
                        self.active_monitor_ref()
                            .is_some_and(|a| a.output() == m.output())
                    }),
                    workspace_ids: self
                        .collection_workspaces(collection.id)
                        .map(|ws| ws.id().get())
                        .collect(),
                }
            })
            .collect()
    }

    pub fn collection_for_output(&self, output: &Output) -> Option<u64> {
        self.monitors()
            .find(|m| m.output() == output)
            .and_then(|m| m.collection_id)
    }

    pub fn collection_workspaces(&self, id: u64) -> impl Iterator<Item = &Workspace<W>> {
        let shown = self
            .monitors()
            .filter(move |m| m.collection_id == Some(id))
            .flat_map(|m| m.workspaces.iter());
        let hidden = self
            .collections
            .iter()
            .filter(move |c| c.id == id)
            .flat_map(|c| c.workspaces.iter());
        shown.chain(hidden)
    }

    pub fn collection_active_workspace(&self, id: u64) -> Option<WorkspaceId> {
        self.monitors()
            .find(|m| m.collection_id == Some(id))
            .map(|m| m.workspaces[m.active_workspace_idx].id())
            .or_else(|| {
                self.collections
                    .iter()
                    .find(|c| c.id == id)
                    .and_then(|c| c.active_workspace)
            })
    }

    fn new_collection(
        &mut self,
        name: Option<String>,
        output: Option<String>,
        persistent: bool,
    ) -> u64 {
        let mut collection = Collection::new(name, output, persistent);
        let workspace = Workspace::new_no_outputs(self.clock.clone(), self.options.clone());
        collection.active_workspace = Some(workspace.id());
        collection.workspaces.push(workspace);
        let id = collection.id;
        self.collections.push(collection);
        id
    }

    pub fn create_collection(&mut self, name: Option<String>) -> u64 {
        if !self.options.layout.collections.enabled {
            return 0;
        }
        let name = name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        if let Some(existing) = name.as_ref().and_then(|name| {
            self.collections
                .iter()
                .find(|c| c.name.as_ref() == Some(name))
        }) {
            let id = existing.id;
            self.focus_collection(&WorkspaceReference::Id(id));
            return id;
        }
        let id = self.new_collection(name.clone(), None, name.is_some());
        if name.is_some() {
            if let Some(collection) = self.collections.iter_mut().find(|c| c.id == id) {
                collection.user_named = true;
            }
        }
        self.focus_collection(&WorkspaceReference::Id(id));
        id
    }

    pub fn set_collection_name(
        &mut self,
        reference: &WorkspaceReference,
        name: Option<String>,
    ) -> bool {
        if !self.options.layout.collections.enabled {
            return false;
        }
        let name = name.map(|name| name.trim().to_owned()).filter(|name| !name.is_empty());
        let Some(index) = self
            .collections
            .iter()
            .enumerate()
            .find(|(i, collection)| collection.matches_reference(reference, *i))
            .map(|(i, _)| i)
        else {
            return false;
        };
        if name.as_ref().is_some_and(|name| {
            self.collections
                .iter()
                .enumerate()
                .any(|(i, collection)| i != index && collection.name.as_ref() == Some(name))
        }) {
            return false;
        }
        let collection = &mut self.collections[index];
        collection.user_named = name.is_some();
        collection.persistent = collection.user_named
            || collection
                .name
                .as_ref()
                .is_some_and(|old| self.options.layout.collections.names.contains(old));
        collection.name = name;
        true
    }

    /// Reconcile configured names without dropping stacks on config reload.
    pub(super) fn configure_collections(&mut self) {
        let names = self.options.layout.collections.names.clone();
        for c in &mut self.collections {
            c.persistent = c.user_named
                || c.name.as_ref().is_some_and(|name| names.contains(name));
        }
        if self.options.layout.collections.enabled {
            for name in names {
                if !self
                    .collections
                    .iter()
                    .any(|c| c.name.as_ref() == Some(&name))
                {
                    self.new_collection(Some(name), None, true);
                }
            }
        }
    }

    pub(super) fn transition_collections(&mut self, enabled: bool) {
        if enabled == self.options.layout.collections.enabled {
            return;
        }
        self.collection_switch = None;
        for monitor in self.monitors_mut() {
            monitor.collection_switch_active = false;
            monitor.collection_switch_offset_x = 0.;
        }
        self.collection_pending_workspaces.clear();
        if enabled {
            let outputs: Vec<_> = self.monitors().map(|m| m.output.clone()).collect();
            for output in outputs {
                let id = self.new_collection(None, Some(output.name()), false);
                let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
                collection.home_output = Some(OutputId::new(&output));
                collection.workspaces.clear();
                self.monitors_mut()
                    .find(|m| m.output() == &output)
                    .unwrap()
                    .collection_id = Some(id);
            }
        } else {
            let mut stacks: Vec<_> = self
                .collections
                .drain(..)
                .flat_map(|c| c.workspaces)
                .collect();
            let old = std::mem::take(&mut self.monitor_set);
            self.monitor_set = match old {
                MonitorSet::Normal {
                    monitors,
                    primary_idx,
                    active_monitor_idx,
                } => {
                    let outputs: Vec<_> = monitors
                        .iter()
                        .map(|m| {
                            (
                                m.output.clone(),
                                m.layout_config.clone(),
                                m.workspaces[m.active_workspace_idx].id(),
                            )
                        })
                        .collect();
                    for monitor in monitors {
                        stacks.extend(monitor.into_workspaces());
                    }
                    let mut assigned: Vec<Vec<Workspace<W>>> =
                        (0..outputs.len()).map(|_| Vec::new()).collect();
                    for ws in stacks.into_iter().filter(|ws| ws.has_windows_or_name()) {
                        let idx = outputs
                            .iter()
                            .position(|(output, _, _)| ws.original_output.matches(output))
                            .unwrap_or(primary_idx);
                        assigned[idx].push(ws);
                    }
                    let monitors = outputs
                        .into_iter()
                        .zip(assigned)
                        .map(|((output, layout, active), stack)| {
                            let mut mon = Monitor::new(
                                output,
                                stack,
                                Some(active),
                                self.clock.clone(),
                                self.options.clone(),
                                layout,
                            );
                            mon.overview_open = self.overview_open;
                            mon.set_overview_progress(self.overview_progress.as_ref());
                            mon
                        })
                        .collect();
                    MonitorSet::Normal {
                        monitors,
                        primary_idx,
                        active_monitor_idx,
                    }
                }
                MonitorSet::NoOutputs { mut workspaces } => {
                    workspaces.extend(stacks.into_iter().filter(|ws| ws.has_windows_or_name()));
                    MonitorSet::NoOutputs { workspaces }
                }
            };
            self.collection_returns.clear();
        }
    }

    fn store_collection_stack(
        &mut self,
        id: u64,
        mut workspaces: Vec<Workspace<W>>,
        active: Option<WorkspaceId>,
    ) {
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

    fn take_collection_monitor(
        &mut self,
        id: u64,
        output: Output,
        layout: Option<LayoutPart>,
    ) -> Monitor<W> {
        let c = self.collections.iter_mut().find(|c| c.id == id).unwrap();
        let stack = std::mem::take(&mut c.workspaces);
        let active = c.active_workspace;
        let mut mon = Monitor::new(
            output.clone(),
            Vec::new(),
            None,
            self.clock.clone(),
            self.options.clone(),
            layout,
        );
        if !stack.is_empty() {
            mon.workspaces = stack;
            mon.active_workspace_idx = active
                .and_then(|id| mon.workspaces.iter().position(|ws| ws.id() == id))
                .unwrap_or(0);
            for ws in &mut mon.workspaces {
                ws.set_output(Some(output.clone()));
                ws.update_config(mon.options.clone());
            }
            if mon
                .workspaces
                .last()
                .is_some_and(|ws| ws.has_windows_or_name())
            {
                mon.add_workspace_bottom();
            }
            if mon.options.layout.empty_workspace_above_first
                && mon
                    .workspaces
                    .first()
                    .is_some_and(|ws| ws.has_windows_or_name())
            {
                mon.add_workspace_top();
            }
            mon.restore_overlay_windows();
            mon.clean_up_workspaces();
        }
        mon.collection_id = Some(id);
        mon.overview_open = self.overview_open;
        mon.set_overview_progress(self.overview_progress.as_ref());
        mon
    }

    pub(super) fn add_collection_output(&mut self, output: Output, layout: Option<LayoutPart>) {
        let had_outputs = self.monitors().next().is_some();
        self.configure_collections();
        let name = output.name();
        let id = self
            .collections
            .iter()
            .find(|c| {
                c.home_output
                    .as_ref()
                    .is_some_and(|home| home.matches(&output))
                    || c.original_output.as_deref() == Some(&name)
            })
            .map(|c| c.id)
            .unwrap_or_else(|| {
                // The first configured collection can serve as the first monitor's desk.
                if self.monitors().next().is_none() {
                    if let Some(c) = self
                        .collections
                        .iter_mut()
                        .find(|c| c.original_output.is_none() && c.persistent)
                    {
                        c.original_output = Some(name.clone());
                        return c.id;
                    }
                }
                self.new_collection(None, Some(name.clone()), false)
            });
        // Only the initial configured workspaces retain upstream startup output routing.
        // Established collection stacks stay intact during subsequent hotplug.
        let pending = self.collection_pending_workspaces.clone();
        let mut arriving = Vec::new();
        for monitor in self.monitors_mut() {
            let mut from_monitor = Vec::new();
            for idx in (0..monitor.workspaces.len()).rev() {
                let ws = &monitor.workspaces[idx];
                if pending.contains(&ws.id()) && ws.original_output.matches(&output) {
                    from_monitor.push(monitor.remove_workspace_by_idx(idx));
                }
            }
            from_monitor.reverse();
            arriving.extend(from_monitor);
        }
        for collection in &mut self.collections {
            if collection.id == id {
                continue;
            }
            let mut idx = 0;
            while idx < collection.workspaces.len() {
                let ws = &collection.workspaces[idx];
                if pending.contains(&ws.id()) && ws.original_output.matches(&output) {
                    arriving.push(collection.workspaces.remove(idx));
                } else {
                    idx += 1;
                }
            }
        }
        if !arriving.is_empty() {
            let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
            if !collection
                .workspaces
                .iter()
                .any(|ws| ws.has_windows_or_name())
            {
                collection.active_workspace = Some(arriving[0].id());
            }
            collection.workspaces.splice(0..0, arriving);
        }
        let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
        collection.original_output = Some(name.clone());
        collection.home_output = Some(OutputId::new(&output));
        // A disconnected monitor's collection may currently be borrowed by another output.
        let borrower = self
            .monitors()
            .find(|m| m.collection_id == Some(id))
            .map(|m| m.output_name().clone());
        if let Some(borrower) = borrower {
            let fallback = self
                .collection_returns
                .remove(&borrower)
                .filter(|return_id| {
                    *return_id != id
                        && self.collections.iter().any(|c| c.id == *return_id)
                        && !self.monitors().any(|m| m.collection_id == Some(*return_id))
                })
                .or_else(|| {
                    // A collection may have been swapped while both outputs were connected,
                    // before the unplug. Reuse the borrower's own hidden home in that case.
                    self.collections
                        .iter()
                        .find(|c| {
                            c.id != id
                                && c.original_output.as_deref() == Some(&borrower)
                                && !self.monitors().any(|m| m.collection_id == Some(c.id))
                        })
                        .map(|c| c.id)
                })
                .unwrap_or_else(|| self.new_collection(None, Some(borrower.clone()), false));
            self.show_collection_on_output(&borrower, fallback);
        }
        let old = std::mem::take(&mut self.monitor_set);
        self.monitor_set = match old {
            MonitorSet::Normal {
                mut monitors,
                primary_idx,
                active_monitor_idx,
            } => {
                monitors.push(self.take_collection_monitor(id, output.clone(), layout));
                MonitorSet::Normal {
                    monitors,
                    primary_idx,
                    active_monitor_idx,
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                // Workspaces configured before the first output belong to its initial desk.
                if !workspaces.is_empty() {
                    let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
                    let focus_initial = !collection.workspaces.iter().any(|ws| ws.has_windows());
                    collection.workspaces.splice(0..0, workspaces);
                    if focus_initial {
                        collection.active_workspace =
                            collection.workspaces.first().map(|ws| ws.id());
                    }
                }
                let mon = self.take_collection_monitor(id, output.clone(), layout);
                MonitorSet::Normal {
                    monitors: vec![mon],
                    primary_idx: 0,
                    active_monitor_idx: 0,
                }
            }
        };
        let assigned: Vec<_> = self
            .collection_workspaces(id)
            .filter(|ws| ws.original_output.matches(&output))
            .map(|ws| ws.id())
            .collect();
        self.collection_pending_workspaces
            .retain(|id| !assigned.contains(id));
        if !had_outputs {
            let detached: Vec<_> = self
                .collections
                .iter()
                .flat_map(|collection| {
                    collection.workspaces.iter().flat_map(|workspace| {
                        workspace
                            .tiles()
                            .filter(|tile| tile.pinned || tile.scratchpad)
                            .map(|tile| tile.window().id().clone())
                    })
                })
                .collect();
            for id in detached {
                let Some(removed) = self.remove_window(&id, Transaction::new()) else {
                    continue;
                };
                let scratchpad = removed.tile.scratchpad;
                if let Some(monitor) = self.monitor_for_output_mut(&output) {
                    monitor.pinned_workspace.add_overlay_tile(
                        removed.tile,
                        WorkspaceAddWindowTarget::Auto,
                        ActivateWindow::No,
                        removed.width,
                        removed.is_full_width,
                    );
                }
                if scratchpad {
                    self.scratchpad_active = Some(id);
                }
            }
        }
    }

    pub(super) fn remove_collection_output(&mut self, output: &Output) {
        if self
            .collection_switch
            .as_ref()
            .is_some_and(|transition| transition.output == *output)
        {
            self.collection_switch = None;
        }
        let MonitorSet::Normal {
            mut monitors,
            mut primary_idx,
            mut active_monitor_idx,
        } = std::mem::take(&mut self.monitor_set)
        else {
            panic!("no outputs");
        };
        let idx = monitors
            .iter()
            .position(|m| m.output() == output)
            .expect("unknown output");
        let mut mon = monitors.remove(idx);
        let id = mon.collection_id.unwrap();
        let active = mon
            .workspaces
            .get(mon.active_workspace_idx)
            .map(|ws| ws.id());
        // Output overlays follow the surviving output, independently of which collection
        // happened to be shown on the disconnected monitor.
        if !monitors.is_empty() {
            let mut overlay = mon.take_pinned_workspace();
            let target_idx = primary_idx.saturating_sub(usize::from(primary_idx >= idx));
            let target = &mut monitors[target_idx];
            let ids: Vec<_> = overlay
                .windows()
                .map(|window| window.id().clone())
                .collect();
            for id in ids {
                let removed = overlay.remove_tile(&id, Transaction::new());
                target.pinned_workspace.add_overlay_tile(
                    removed.tile,
                    WorkspaceAddWindowTarget::Auto,
                    ActivateWindow::No,
                    removed.width,
                    removed.is_full_width,
                );
            }
        }
        self.store_collection_stack(id, mon.into_collection_workspaces(), active);
        primary_idx = primary_idx.saturating_sub(usize::from(primary_idx >= idx));
        active_monitor_idx =
            active_monitor_idx.saturating_sub(usize::from(active_monitor_idx >= idx));
        self.monitor_set = if monitors.is_empty() {
            MonitorSet::NoOutputs { workspaces: vec![] }
        } else {
            MonitorSet::Normal {
                monitors,
                primary_idx,
                active_monitor_idx,
            }
        };
        self.collection_returns.remove(&output.name());
        self.clean_up_collections();
    }

    fn show_collection_on_output(&mut self, output_name: &str, id: u64) {
        let Some((old_id, output, layout)) = self
            .monitors()
            .find(|m| m.output_name() == output_name)
            .map(|m| (m.collection_id, m.output.clone(), m.layout_config.clone()))
        else {
            return;
        };
        if old_id == Some(id) {
            return;
        }
        let other = self
            .monitors()
            .find(|m| m.collection_id == Some(id))
            .map(|m| m.output_name().clone());
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
        let mut pinned = None;
        if let Some(mon) = self.monitors_mut().find(|m| m.output_name() == output_name) {
            active = mon
                .workspaces
                .get(mon.active_workspace_idx)
                .map(|ws| ws.id());
            stack = std::mem::take(&mut mon.workspaces);
            pinned = Some((mon.take_pinned_workspace(), mon.pinned_is_active));
        }
        if let Some(old) = old_id {
            self.store_collection_stack(old, stack, active);
        }
        let mut new_monitor = self.take_collection_monitor(id, output, layout);
        if let Some((pinned, was_active)) = pinned {
            new_monitor.set_pinned_workspace(pinned);
            new_monitor.pinned_is_active = was_active;
        }
        if let Some(mon) = self.monitors_mut().find(|m| m.output_name() == output_name) {
            *mon = new_monitor;
        }
    }

    pub fn focus_collection(&mut self, reference: &WorkspaceReference) {
        if let Some(transition) = self.collection_switch.take() {
            if let Some(monitor) = self.monitor_for_output_mut(&transition.output) {
                monitor.collection_switch_active = false;
                monitor.collection_switch_offset_x = 0.;
            }
        }
        let Some(output) = self.active_output().cloned() else {
            return;
        };
        self.focus_collection_on_output(&output, reference);
    }

    pub fn focus_collection_on_output(
        &mut self,
        output: &Output,
        reference: &WorkspaceReference,
    ) {
        if !self.options.layout.collections.enabled {
            return;
        }
        let Some(id) = self
            .collections
            .iter()
            .enumerate()
            .find(|(i, c)| c.matches_reference(reference, *i))
            .map(|(_, c)| c.id)
        else {
            return;
        };
        let Some((name, old)) = self
            .monitors()
            .find(|monitor| monitor.output() == output)
            .map(|m| (m.output_name().clone(), m.collection_id))
        else {
            return;
        };
        // Focusing the already shown desk is a no-op. In particular, do not record a borrowed
        // desk as its own return target.
        if old == Some(id) {
            return;
        }
        let borrowed = self
            .collections
            .iter()
            .find(|c| c.id == id)
            .is_some_and(|c| {
                c.home_output
                    .as_ref()
                    .is_some_and(|home| !self.monitors().any(|m| home.matches(m.output())))
            });
        if borrowed {
            if let Some(old) = old {
                self.collection_returns.entry(name.clone()).or_insert(old);
            }
        } else {
            self.collection_returns.remove(&name);
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

    fn adjacent_collection_from(&self, current: u64, next: bool) -> Option<u64> {
        let idx = self.collections.iter().position(|collection| collection.id == current)?;
        let len = self.collections.len();
        (len > 1).then(|| self.collections[(idx + if next { 1 } else { len - 1 }) % len].id)
    }

    pub fn collection_switch_gesture_begin(&mut self, output: &Output) -> bool {
        if !self.options.layout.collections.enabled || self.overview_open {
            return false;
        }
        let Some(current_id) = self
            .monitor_for_output(output)
            .and_then(|monitor| monitor.collection_id)
        else {
            return false;
        };
        let previous = self.collection_switch.take();
        let (base, other_id, side, other_offset_delta) = if let Some(previous) = previous {
            if previous.output == *output {
                (
                    previous.offset(),
                    previous.other_id,
                    previous.side,
                    previous.other_offset_delta,
                )
            } else {
                if let Some(monitor) = self.monitor_for_output_mut(&previous.output) {
                    monitor.collection_switch_offset_x = 0.;
                    monitor.collection_switch_active = false;
                }
                (0., None, 0., 0.)
            }
        } else {
            (0., None, 0., 0.)
        };
        self.collection_switch = Some(CollectionSwitchTransition {
            output: output.clone(),
            current_id,
            other_id,
            other_offset_delta,
            side,
            offset: CollectionSwitchOffset::Gesture {
                base,
                tracker: SwipeTracker::new(),
            },
        });
        true
    }

    pub fn collection_switch_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
    ) -> Option<Output> {
        let transition = self.collection_switch.as_mut()?;
        let CollectionSwitchOffset::Gesture { base, tracker } = &mut transition.offset else {
            return None;
        };
        tracker.push(delta_x, timestamp);
        let movement = tracker.pos();
        let width = output_size(&transition.output).w;
        let offset = (*base + movement * width / 300.).clamp(-width, width);
        if offset.abs() >= 1. {
            let next = offset < 0.;
            let side = if next { 1. } else { -1. };
            let index = self
                .collections
                .iter()
                .position(|collection| collection.id == transition.current_id);
            transition.other_id = index.and_then(|index| {
                let len = self.collections.len();
                (len > 1).then(|| self.collections[(index + if next { 1 } else { len - 1 }) % len].id)
            });
            transition.side = side;
            transition.other_offset_delta = side * width;
        }
        let output = transition.output.clone();
        if let Some(monitor) = self.monitor_for_output_mut(&output) {
            monitor.collection_switch_offset_x = offset;
            monitor.collection_switch_active = true;
        }
        Some(output)
    }

    pub fn collection_switch_gesture_end(&mut self, cancelled: bool) -> Option<Output> {
        let mut transition = self.collection_switch.take()?;
        let offset = transition.offset();
        let CollectionSwitchOffset::Gesture { base, mut tracker } = transition.offset else {
            self.collection_switch = Some(transition);
            return None;
        };
        tracker.push(0., self.clock.now_unadjusted());
        let output = transition.output.clone();
        let width = output_size(&output).w;
        let projected = (base + tracker.projected_end_pos() * width / 300.).clamp(-width, width);
        let commit = !cancelled
            && transition.other_id.is_some()
            && ((transition.side > 0. && projected <= -width * 0.45)
                || (transition.side < 0. && projected >= width * 0.45));
        if commit {
            let target = transition.other_id.unwrap();
            let source = transition.current_id;
            self.focus_collection_on_output(&output, &WorkspaceReference::Id(target));
            transition.current_id = target;
            transition.other_id = Some(source);
            transition.other_offset_delta = -transition.side * width;
            transition.offset = CollectionSwitchOffset::Animation(Animation::new(
                self.clock.clone(),
                offset + transition.side * width,
                0.,
                tracker.velocity() * width / 300.,
                self.options.animations.workspace_switch.0,
            ));
        } else {
            transition.other_offset_delta = transition.side * width;
            transition.offset = CollectionSwitchOffset::Animation(Animation::new(
                self.clock.clone(),
                offset,
                0.,
                tracker.velocity() * width / 300.,
                self.options.animations.workspace_switch.0,
            ));
        }
        let done = matches!(&transition.offset, CollectionSwitchOffset::Animation(animation) if animation.is_done());
        if done {
            if let Some(monitor) = self.monitor_for_output_mut(&output) {
                monitor.collection_switch_offset_x = 0.;
                monitor.collection_switch_active = false;
            }
        } else {
            self.collection_switch = Some(transition);
        }
        Some(output)
    }

    pub fn collection_switch_render_workspace(
        &self,
        output: &Output,
    ) -> Option<(&Workspace<W>, f64)> {
        let transition = self.collection_switch.as_ref()?;
        if transition.output != *output {
            return None;
        }
        let id = transition.other_id?;
        let workspace_id = self.collection_active_workspace(id)?;
        let workspace = self.collection_workspaces(id).find(|ws| ws.id() == workspace_id)?;
        Some((workspace, transition.offset() + transition.other_offset_delta))
    }

    pub(super) fn update_collection_switch_render_state(&mut self) {
        if self.collection_switch.as_ref().is_some_and(|transition| {
            matches!(&transition.offset, CollectionSwitchOffset::Animation(animation) if animation.is_done())
        }) {
            self.collection_switch = None;
        }
        let transition = self
            .collection_switch
            .as_ref()
            .map(|transition| (transition.output.clone(), transition.offset()));
        if let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set {
            for monitor in monitors {
                if let Some((output, offset)) = &transition {
                    if monitor.output() == output {
                        monitor.collection_switch_active = true;
                        monitor.collection_switch_offset_x = *offset;
                        continue;
                    }
                }
                monitor.collection_switch_active = false;
                monitor.collection_switch_offset_x = 0.;
            }
        }
    }
    pub fn focus_collection_next(&mut self) {
        if let Some(id) = self.adjacent_collection(true) {
            self.focus_collection(&WorkspaceReference::Id(id));
        }
    }
    pub fn focus_collection_previous(&mut self) {
        if let Some(id) = self.adjacent_collection(false) {
            self.focus_collection(&WorkspaceReference::Id(id));
        }
    }
    pub fn move_window_to_collection_next(&mut self) {
        if let Some(id) = self.adjacent_collection(true) {
            self.move_window_to_collection(&WorkspaceReference::Id(id));
        }
    }
    pub fn move_window_to_collection_previous(&mut self) {
        if let Some(id) = self.adjacent_collection(false) {
            self.move_window_to_collection(&WorkspaceReference::Id(id));
        }
    }

    pub fn move_window_to_collection(&mut self, reference: &WorkspaceReference) {
        if !self.options.layout.collections.enabled {
            return;
        }
        let Some(id) = self
            .collections
            .iter()
            .enumerate()
            .find(|(i, c)| c.matches_reference(reference, *i))
            .map(|(_, c)| c.id)
        else {
            return;
        };
        if self
            .active_monitor_ref()
            .is_some_and(|m| m.collection_id == Some(id))
        {
            return;
        }
        let Some(window) = self
            .active_workspace()
            .and_then(|ws| ws.active_window())
            .map(|w| w.id().clone())
        else {
            return;
        };
        let Some(removed) = self.remove_window(&window, Transaction::new()) else {
            return;
        };
        let shown = self.monitors().any(|m| m.collection_id == Some(id));
        if shown {
            let mon = self
                .monitors_mut()
                .find(|m| m.collection_id == Some(id))
                .unwrap();
            mon.add_tile(
                removed.tile,
                MonitorAddWindowTarget::Auto,
                ActivateWindow::No,
                false,
                removed.width,
                removed.is_full_width,
                removed.is_floating,
            );
        } else {
            let c = self.collections.iter_mut().find(|c| c.id == id).unwrap();
            if c.workspaces.is_empty() {
                c.workspaces.push(Workspace::new_with_config_no_outputs(
                    None,
                    self.clock.clone(),
                    self.options.clone(),
                ));
            }
            let idx = c
                .active_workspace
                .and_then(|id| c.workspaces.iter().position(|ws| ws.id() == id))
                .unwrap_or(0);
            let ws = &mut c.workspaces[idx];
            ws.add_tile(
                removed.tile,
                WorkspaceAddWindowTarget::Auto,
                ActivateWindow::No,
                removed.width,
                removed.is_full_width,
                removed.is_floating,
            );
            ws.refresh(false, false);
            if c.workspaces
                .last()
                .is_some_and(|ws| ws.has_windows_or_name())
            {
                c.workspaces.push(Workspace::new_no_outputs(
                    self.clock.clone(),
                    self.options.clone(),
                ));
            }
        }
        self.clean_up_collections();
    }

    pub(super) fn clean_up_collections(&mut self) {
        let shown: Vec<_> = self.monitors().filter_map(|m| m.collection_id).collect();
        let outputs: Vec<_> = self.monitors().map(|m| m.output().clone()).collect();
        let returns: Vec<_> = self.collection_returns.values().copied().collect();
        self.collections.retain(|c| {
            c.persistent
                || shown.contains(&c.id)
                || returns.contains(&c.id)
                || (c.name.is_none()
                    && c.home_output
                        .as_ref()
                        .is_some_and(|home| outputs.iter().any(|output| home.matches(output))))
                || c.workspaces.iter().any(|ws| ws.has_windows())
        });
    }
}
