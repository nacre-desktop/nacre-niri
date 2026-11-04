//! Atomic operations across the physical monitor ring.

use super::monitor::Monitor;
use super::workspace::OutputId;
use super::{Layout, LayoutElement, MonitorSet};

impl<W: LayoutElement> Layout<W> {
    /// Monitor indices in physical order, independent of hotplug/insertion order.
    pub(super) fn physical_monitor_ring(&self) -> Vec<usize> {
        let MonitorSet::Normal { monitors, .. } = &self.monitor_set else {
            return Vec::new();
        };
        let mut ring: Vec<_> = (0..monitors.len()).collect();
        ring.sort_by_key(|&i| {
            let location = monitors[i].output().current_location();
            (location.x, location.y, monitors[i].output_name().clone())
        });
        ring
    }

    /// Exchange the selected workspaces with the adjacent physical monitor. Focus stays on the
    /// same output, and workspace identities and names travel with their contents.
    pub fn swap_workspace_with_monitor(&mut self, next: bool) {
        let ring = self.physical_monitor_ring();
        if ring.len() < 2 {
            return;
        }
        self.cancel_collection_switch();
        let MonitorSet::Normal { monitors, active_monitor_idx, .. } = &mut self.monitor_set else {
            return;
        };
        let position = ring.iter().position(|i| i == active_monitor_idx).unwrap();
        let step = if next { 1 } else { ring.len() - 1 };
        let target = ring[(position + step) % ring.len()];
        let source = *active_monitor_idx;
        let (source, target) = two_monitors(monitors, source, target);
        std::mem::swap(
            &mut source.workspaces[source.active_workspace_idx],
            &mut target.workspaces[target.active_workspace_idx],
        );
        for monitor in [source, target] {
            monitor.workspaces[monitor.active_workspace_idx].original_output =
                OutputId::new(monitor.output());
            // The previous selection may now live on the other monitor.
            monitor.previous_workspace_id = None;
            rebind_stack(monitor);
        }
    }

    /// Rotate the shown workspace sets, explicitly adopting their destination outputs as home.
    /// Collection identities, hidden desks, pinned overlays, and physical focus stay in place.
    pub fn rotate_workspace_sets(&mut self, next: bool) {
        if !self.options.layout.collections.enabled {
            return;
        }
        let ring = self.physical_monitor_ring();
        if ring.len() < 2 {
            return;
        }
        self.cancel_collection_switch();
        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            return;
        };
        // Swapping the first stack with each successive stack is one cyclic permutation.
        let order: Vec<_> = if next {
            ring[1..].to_vec()
        } else {
            ring[1..].iter().rev().copied().collect()
        };
        for target in order {
            let (a, b) = two_monitors(monitors, ring[0], target);
            std::mem::swap(&mut a.workspaces, &mut b.workspaces);
            std::mem::swap(&mut a.active_workspace_idx, &mut b.active_workspace_idx);
            std::mem::swap(&mut a.previous_workspace_id, &mut b.previous_workspace_id);
        }
        for monitor in monitors {
            for workspace in &mut monitor.workspaces {
                workspace.original_output = OutputId::new(&monitor.output);
            }
            rebind_stack(monitor);
        }
    }
}

fn two_monitors<W: LayoutElement>(
    monitors: &mut [Monitor<W>],
    a: usize,
    b: usize,
) -> (&mut Monitor<W>, &mut Monitor<W>) {
    if a < b {
        let (left, right) = monitors.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = monitors.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}

fn rebind_stack<W: LayoutElement>(monitor: &mut Monitor<W>) {
    monitor.workspace_switch = None;
    for workspace in &mut monitor.workspaces {
        workspace.set_output(Some(monitor.output.clone()));
        workspace.update_config(monitor.options.clone());
    }
    // Boundary rows may differ between output-specific layouts, or a selected empty boundary
    // may have received a populated workspace. Preserve slots except for required boundary rows.
    if monitor.workspaces.last().unwrap().has_workspace_contents() {
        monitor.add_workspace_bottom();
    }
    if monitor.options.layout.empty_workspace_above_first
        && monitor.workspaces.first().unwrap().has_workspace_contents()
    {
        monitor.add_workspace_top();
    }
    monitor.clean_up_workspaces();
}
