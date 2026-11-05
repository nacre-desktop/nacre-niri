use super::monitor::MonitorAddWindowTarget;
use super::workspace::WorkspaceAddWindowTarget;
use super::{ActivateWindow, Layout, LayoutElement};
use crate::utils::transaction::Transaction;

impl<W: LayoutElement> Layout<W> {
    fn restore_removed_to_active_workspace(
        &mut self,
        removed: super::RemovedTile<W>,
        is_floating: bool,
    ) {
        if let Some(monitor) = self.active_monitor() {
            monitor.add_tile(
                removed.tile,
                MonitorAddWindowTarget::Auto,
                ActivateWindow::No,
                false,
                removed.width,
                removed.is_full_width,
                is_floating,
            );
            return;
        }
        if let super::MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
            if workspaces.is_empty() {
                workspaces.push(super::Workspace::new_no_outputs(
                    self.clock.clone(),
                    self.options.clone(),
                ));
            }
            workspaces[0].add_tile(
                removed.tile,
                WorkspaceAddWindowTarget::Auto,
                ActivateWindow::No,
                removed.width,
                removed.is_full_width,
                is_floating,
            );
        }
    }

    pub(super) fn disable_scratchpad(&mut self) {
        let hidden: Vec<_> = self
            .scratchpad
            .windows()
            .map(|window| window.id().clone())
            .collect();
        for id in hidden {
            let mut removed = self.scratchpad.remove_tile(&id, Transaction::new());
            removed.tile.scratchpad = false;
            let floating = removed.is_floating;
            self.restore_removed_to_active_workspace(removed, floating);
        }
        for monitor in self.monitors_mut() {
            let visible: Vec<_> = monitor
                .pinned_workspace
                .tiles()
                .filter(|tile| tile.scratchpad)
                .map(|tile| tile.window().id().clone())
                .collect();
            for id in visible {
                let removed = monitor
                    .pinned_workspace
                    .remove_tile(&id, Transaction::new());
                let mut tile = removed.tile;
                tile.scratchpad = false;
                tile.pinned = false;
                monitor.add_tile(
                    tile,
                    MonitorAddWindowTarget::Auto,
                    ActivateWindow::No,
                    false,
                    removed.width,
                    removed.is_full_width,
                    true,
                );
            }
        }
        for monitor in self.monitors_mut() {
            monitor.pinned_is_active &= monitor.pinned_workspace.has_windows();
        }
        for workspace in self.workspaces_mut() {
            for tile in workspace.tiles_mut() {
                if tile.scratchpad {
                    tile.scratchpad = false;
                }
            }
        }
        if let Some(super::InteractiveMoveState::Moving(moving)) = &mut self.interactive_move {
            moving.tile.scratchpad = false;
        }
        self.scratchpad_active = None;
        self.scratchpad_cycle.clear();
        self.scratchpad_origins.clear();
    }

    pub(super) fn disable_window_pinning(&mut self) {
        if let Some(super::InteractiveMoveState::Moving(moving)) = &mut self.interactive_move {
            moving.tile.pinned = false;
        }
        for monitor in self.monitors_mut() {
            let pinned: Vec<_> = monitor
                .pinned_workspace
                .tiles()
                .filter(|tile| tile.pinned)
                .map(|tile| tile.window().id().clone())
                .collect();
            for id in pinned {
                let removed = monitor
                    .pinned_workspace
                    .remove_tile(&id, Transaction::new());
                let mut tile = removed.tile;
                tile.pinned = false;
                tile.scratchpad = false;
                monitor.add_tile(
                    tile,
                    MonitorAddWindowTarget::Auto,
                    ActivateWindow::No,
                    false,
                    removed.width,
                    removed.is_full_width,
                    true,
                );
            }
            monitor.pinned_is_active &= monitor.pinned_workspace.has_windows();
        }
        for workspace in self.workspaces_mut() {
            for tile in workspace.tiles_mut() {
                tile.pinned = false;
            }
        }
    }

    /// Move a window into the hidden scratchpad. `None` means the focused window.
    pub fn move_window_to_scratchpad(&mut self, id: Option<&W::Id>) {
        self.hide_in_scratchpad(id, true);
    }

    fn hide_in_scratchpad(&mut self, id: Option<&W::Id>, update_recency: bool) {
        if !self.options.nacre.scratchpad {
            return;
        }
        let id = id.cloned().or_else(|| {
            self.focus().map(|window| window.id().clone()).or_else(|| {
                self.active_workspace()?
                    .active_window()
                    .map(|window| window.id().clone())
            })
        });
        let Some(id) = id else {
            return;
        };
        // Preserve the original workspace when a shown scratchpad overlay is hidden again.
        // Ordinary windows have a stable workspace ID even when its collection is hidden.
        let origin = self
            .scratchpad_origins
            .iter()
            .find(|(window, _, _)| window == &id)
            .filter(|_| {
                self.scratchpad.has_window(&id) || self.scratchpad_active.as_ref() == Some(&id)
            })
            .cloned()
            .or_else(|| {
                self.workspaces()
                    .find(|(_, _, workspace)| workspace.has_window(&id))
                    .map(|(_, _, workspace)| {
                        (id.clone(), workspace.id(), workspace.is_floating(&id))
                    })
            })
            .or_else(|| {
                self.active_workspace()
                    .map(|workspace| (id.clone(), workspace.id(), true))
            });
        let Some(mut removed) = self.remove_window(&id, Transaction::new()) else {
            return;
        };
        if let Some(origin) = origin {
            self.scratchpad_origins.push(origin);
        }
        removed.tile.scratchpad = false;
        removed.tile.pinned = false;
        self.scratchpad.add_tile(
            removed.tile,
            WorkspaceAddWindowTarget::Auto,
            ActivateWindow::No,
            removed.width,
            removed.is_full_width,
            removed.is_floating,
        );
        self.scratchpad.refresh(false, false);
        if update_recency {
            self.scratchpad_cycle.retain(|candidate| candidate != &id);
        }
        if !self.scratchpad_cycle.contains(&id) {
            self.scratchpad_cycle.push(id.clone());
        }
        if self.scratchpad_active.as_ref() == Some(&id) {
            self.scratchpad_active = None;
        }
    }

    /// Show a hidden scratchpad window as a centered floating window on the focused output.
    pub fn scratchpad_show_window(&mut self, id: &W::Id) {
        if !self.options.nacre.scratchpad
            || self.active_monitor().is_none()
            || !self.scratchpad.has_window(id)
        {
            return;
        }
        if self
            .scratchpad_active
            .as_ref()
            .is_some_and(|active| active != id)
        {
            let active = self.scratchpad_active.clone().unwrap();
            self.hide_in_scratchpad(Some(&active), false);
        }
        let mut removed = self.scratchpad.remove_tile(id, Transaction::new());
        let Some(monitor) = self.active_monitor() else {
            self.scratchpad.add_tile(
                removed.tile,
                WorkspaceAddWindowTarget::Auto,
                ActivateWindow::No,
                removed.width,
                removed.is_full_width,
                removed.is_floating,
            );
            return;
        };
        removed.tile.scratchpad = true;
        monitor.pinned_workspace.add_overlay_tile(
            removed.tile,
            WorkspaceAddWindowTarget::Auto,
            ActivateWindow::Yes,
            removed.width,
            removed.is_full_width,
        );
        monitor.pinned_workspace.center_window(Some(id));
        monitor.pinned_is_active = true;
        self.scratchpad_active = Some(id.clone());
    }

    /// Cycle through scratchpad windows, hiding the current one when it is the last remaining.
    pub fn toggle_scratchpad(&mut self) {
        if !self.options.nacre.scratchpad {
            return;
        }
        let live: Vec<_> = self
            .windows()
            .map(|(_, window)| window.id().clone())
            .collect();
        self.scratchpad_cycle.retain(|id| live.contains(id));
        let visible = self.scratchpad_active.clone();
        let Some(next) = self.next_scratchpad_id(visible.as_ref()) else {
            if let Some(visible) = visible {
                self.hide_in_scratchpad(Some(&visible), false);
            }
            return;
        };
        if let Some(visible) = visible {
            self.hide_in_scratchpad(Some(&visible), false);
        }
        self.scratchpad_show_window(&next);
    }

    /// Restore the newest hidden window to its source workspace, then focus it.
    /// If an unnamed source workspace has expired, use the active workspace instead.
    pub fn restore_last_hidden_window(&mut self) {
        if !self.options.nacre.scratchpad || !self.options.nacre.window_restore {
            return;
        }
        let newest = self
            .scratchpad_cycle
            .iter()
            .rev()
            .find(|id| self.scratchpad.has_window(id))
            .cloned();
        if let Some(id) = newest {
            self.restore_hidden_window(&id, true);
        }
    }

    /// Restore all hidden windows originating on the current workspace without changing focus.
    pub fn restore_current_workspace_hidden_windows(&mut self) {
        if !self.options.nacre.scratchpad || !self.options.nacre.window_restore {
            return;
        }
        let Some(current) = self.active_workspace().map(|workspace| workspace.id()) else {
            return;
        };
        let ids: Vec<_> = self
            .scratchpad_origins
            .iter()
            .filter(|(id, source, _)| *source == current && self.scratchpad.has_window(id))
            .map(|(id, _, _)| id.clone())
            .collect();
        for id in ids {
            self.restore_hidden_window(&id, false);
        }
    }

    fn restore_hidden_window(&mut self, id: &W::Id, focus: bool) {
        if !self.scratchpad.has_window(id) {
            return;
        }
        let origin = self
            .scratchpad_origins
            .iter()
            .find(|(window, _, _)| window == id);
        let target =
            origin.and_then(|(_, source, _)| self.find_workspace_by_id(*source).map(|_| *source));
        let floating = origin.map(|(_, _, floating)| *floating);
        let mut removed = self.scratchpad.remove_tile(id, Transaction::new());
        removed.tile.scratchpad = false;
        removed.tile.pinned = false;
        self.scratchpad_cycle.retain(|candidate| candidate != id);
        self.scratchpad_origins
            .retain(|(window, _, _)| window != id);
        let is_floating = floating.unwrap_or(removed.is_floating);
        if let Some(target) = target {
            let output = self
                .monitors()
                .find(|monitor| {
                    monitor
                        .workspaces
                        .iter()
                        .any(|workspace| workspace.id() == target)
                })
                .map(|monitor| monitor.output.clone());
            if let Some(output) = output {
                let monitor = self.monitor_for_output_mut(&output).unwrap();
                monitor.add_tile(
                    removed.tile,
                    MonitorAddWindowTarget::Workspace {
                        id: target,
                        column_idx: None,
                    },
                    ActivateWindow::No,
                    false,
                    removed.width,
                    removed.is_full_width,
                    is_floating,
                );
            } else {
                // Detached collections and the no-output stack have no monitor. In particular,
                // do not unwrap current_output() merely to restore an ordinary window.
                let workspace = self
                    .workspaces_mut()
                    .find(|workspace| workspace.id() == target)
                    .unwrap();
                workspace.add_tile(
                    removed.tile,
                    WorkspaceAddWindowTarget::Auto,
                    ActivateWindow::No,
                    removed.width,
                    removed.is_full_width,
                    is_floating,
                );
            }
        } else {
            self.restore_removed_to_active_workspace(removed, is_floating);
        }
        if focus {
            self.activate_window(id);
        }
    }

    fn next_scratchpad_id(&self, after: Option<&W::Id>) -> Option<W::Id> {
        if self.scratchpad_cycle.is_empty() {
            return self
                .scratchpad
                .windows()
                .last()
                .map(|window| window.id().clone());
        }
        let end = after
            .and_then(|id| {
                self.scratchpad_cycle
                    .iter()
                    .position(|candidate| candidate == id)
            })
            .unwrap_or(self.scratchpad_cycle.len());
        self.scratchpad_cycle[..end]
            .iter()
            .rev()
            .find(|id| self.scratchpad.has_window(id))
            .cloned()
    }

    pub(super) fn release_scratchpad_overlay(&mut self, id: &W::Id) {
        let output = self
            .monitors()
            .find(|monitor| {
                monitor
                    .pinned_workspace
                    .tiles()
                    .any(|tile| tile.window().id() == id && tile.scratchpad)
            })
            .map(|monitor| monitor.output.clone());
        let Some(output) = output else { return };
        let focused = self.focus().is_some_and(|window| window.id() == id);
        let Some(mut removed) = self.remove_window(id, Transaction::new()) else {
            return;
        };
        removed.tile.scratchpad = false;
        self.scratchpad_cycle.retain(|candidate| candidate != id);
        if let Some(monitor) = self.monitor_for_output_mut(&output) {
            monitor.add_tile(
                removed.tile,
                MonitorAddWindowTarget::Auto,
                if focused {
                    ActivateWindow::Yes
                } else {
                    ActivateWindow::No
                },
                false,
                removed.width,
                removed.is_full_width,
                true,
            );
            if focused {
                monitor.pinned_is_active = false;
            }
        }
    }

    pub(super) fn is_window_in_overlay(&self, id: &W::Id) -> bool {
        self.monitors()
            .any(|monitor| monitor.pinned_workspace.has_window(id))
    }

    pub fn is_window_hidden(&self, id: &W::Id) -> bool {
        self.scratchpad.has_window(id)
    }

    pub fn scratchpad_windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.scratchpad.windows()
    }

    pub fn toggle_window_pinned(&mut self, id: Option<&W::Id>) {
        if !self.options.nacre.window_pinning {
            return;
        }
        let Some(id) = id.cloned().or_else(|| {
            self.focus().map(|window| window.id().clone()).or_else(|| {
                self.active_workspace()?
                    .active_window()
                    .map(|window| window.id().clone())
            })
        }) else {
            return;
        };
        if self.is_window_hidden(&id) {
            return;
        }
        if self.scratchpad_active.as_ref() == Some(&id) {
            self.scratchpad_active = None;
        }
        self.scratchpad_cycle.retain(|candidate| candidate != &id);
        let was_focused = self.focus().is_some_and(|window| window.id() == &id);
        if let Some(moving) = self
            .interactive_move
            .as_ref()
            .and_then(|state| state.moving())
            .filter(|moving| moving.tile.window().id() == &id)
        {
            if !moving.is_floating {
                self.toggle_window_floating(Some(&id));
            }
            if let Some(super::InteractiveMoveState::Moving(moving)) = &mut self.interactive_move {
                moving.tile.pinned = !moving.tile.pinned;
                moving.tile.scratchpad = false;
            }
            return;
        }
        if self.is_window_pinned(&id) {
            let focused_id = self.focus().map(|window| window.id().clone());
            let output_and_active = self
                .monitors()
                .find(|monitor| monitor.pinned_workspace.has_window(&id))
                .map(|monitor| (monitor.output.clone(), monitor.pinned_is_active));
            if output_and_active.is_none() {
                for workspace in self.workspaces_mut() {
                    if workspace.has_window(&id) {
                        if let Some(tile) =
                            workspace.tiles_mut().find(|tile| tile.window().id() == &id)
                        {
                            tile.pinned = false;
                        }
                        return;
                    }
                }
                return;
            }
            let Some(mut removed) = self.remove_window(&id, Transaction::new()) else {
                return;
            };
            removed.tile.pinned = false;
            removed.tile.scratchpad = false;
            let (output, was_overlay_focused) = output_and_active.unwrap();
            if let Some(monitor) = self.monitors_mut().find(|monitor| monitor.output == output) {
                monitor.add_tile(
                    removed.tile,
                    MonitorAddWindowTarget::Auto,
                    if was_focused {
                        ActivateWindow::Yes
                    } else {
                        ActivateWindow::No
                    },
                    false,
                    removed.width,
                    removed.is_full_width,
                    true,
                );
                monitor.pinned_is_active = was_overlay_focused && focused_id.as_ref() != Some(&id);
            }
            return;
        }

        let output = self
            .monitors()
            .find(|monitor| monitor.has_window(&id))
            .map(|monitor| monitor.output.clone());
        let Some(output) = output else { return };
        let overlay_was_focused = self
            .monitors()
            .find(|monitor| monitor.output == output)
            .is_some_and(|monitor| monitor.pinned_is_active);
        if let Some(workspace) = self
            .monitors_mut()
            .find(|monitor| monitor.output == output)
            .and_then(|monitor| {
                monitor
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.has_window(&id))
            })
        {
            let mode = workspace
                .tiles()
                .find(|tile| tile.window().id() == &id)
                .map(|tile| tile.window().pending_sizing_mode());
            if mode.is_some_and(|mode| mode.is_fullscreen()) {
                workspace.set_fullscreen(&id, false);
            } else if mode.is_some_and(|mode| mode.is_maximized()) {
                workspace.set_maximized(&id, false);
            }
            if !workspace.is_floating(&id) {
                workspace.toggle_window_floating(Some(&id));
            }
        }
        let Some(mut removed) = self.remove_window(&id, Transaction::new()) else {
            return;
        };
        removed.tile.pinned = true;
        removed.tile.scratchpad = false;
        if let Some(monitor) = self.monitors_mut().find(|monitor| monitor.output == output) {
            monitor.pinned_workspace.add_overlay_tile(
                removed.tile,
                WorkspaceAddWindowTarget::Auto,
                if was_focused {
                    ActivateWindow::Yes
                } else {
                    ActivateWindow::No
                },
                removed.width,
                removed.is_full_width,
            );
            monitor.pinned_is_active = was_focused || overlay_was_focused;
        }
    }

    pub fn is_window_pinned(&self, id: &W::Id) -> bool {
        self.interactive_move
            .as_ref()
            .and_then(|state| state.moving())
            .is_some_and(|moving| moving.tile.window().id() == id && moving.tile.pinned)
            || self.monitors().any(|monitor| {
                monitor
                    .pinned_workspace
                    .tiles()
                    .any(|tile| tile.window().id() == id && tile.pinned)
            })
            || self.workspaces().any(|(_, _, workspace)| {
                workspace
                    .tiles()
                    .any(|tile| tile.window().id() == id && tile.pinned)
            })
    }
}
