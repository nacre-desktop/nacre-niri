use super::*;

fn layout(collections: bool, empty_above: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.collections.enabled = collections;
    options.layout.empty_workspace_above_first = empty_above;
    Layout::with_options(Clock::with_time(Duration::ZERO), options)
}

fn monitor(layout: &Layout<TestWindow>, id: usize) -> &Monitor<TestWindow> {
    layout.monitors().find(|m| m.output_name() == &format!("output{id}")).unwrap()
}

fn selected(layout: &Layout<TestWindow>, id: usize) -> (WorkspaceId, usize) {
    let m = monitor(layout, id);
    (m.workspaces[m.active_workspace_idx].id(), m.active_workspace_idx)
}

fn populate(layout: &mut Layout<TestWindow>, count: usize) {
    for id in 1..=count {
        Op::AddOutput(id).apply(layout);
        Op::FocusOutput(id).apply(layout);
        Op::AddWindow { params: TestWindowParams::new(id) }.apply(layout);
        Op::CompleteAnimations.apply(layout);
    }
    Op::FocusOutput(1).apply(layout);
}

#[test]
fn monitor_workspace_exchange_roundtrip_preserves_slots_names_and_physical_focus() {
    for collections in [false, true] {
        for empty_above in [false, true] {
            let mut layout = layout(collections, empty_above);
            populate(&mut layout, 2);
            for id in 1..=2 {
                let m = layout.monitors_mut().find(|m| m.output_name() == &format!("output{id}")).unwrap();
                m.workspaces[m.active_workspace_idx].name = Some(format!("selected{id}"));
            }
            let before = [selected(&layout, 1), selected(&layout, 2)];
            let collections_before: Vec<_> = layout.monitors().map(|m| m.collection_id).collect();
            layout.swap_workspace_with_monitor(true);
            assert_eq!(selected(&layout, 1), before[1]);
            assert_eq!(selected(&layout, 2), before[0]);
            assert_eq!(layout.active_output().unwrap().name(), "output1");
            assert_eq!(monitor(&layout, 1).workspaces[before[0].1].name.as_deref(), Some("selected2"));
            assert_eq!(layout.monitors().map(|m| m.collection_id).collect::<Vec<_>>(), collections_before);
            layout.verify_invariants();
            layout.swap_workspace_with_monitor(false);
            assert_eq!([selected(&layout, 1), selected(&layout, 2)], before);
            layout.verify_invariants();
        }
    }
}

#[test]
fn monitor_workspace_exchange_of_empty_boundary_preserves_all_windows() {
    for collections in [false, true] {
        for empty_above in [false, true] {
            let mut layout = layout(collections, empty_above);
            populate(&mut layout, 1);
            Op::AddOutput(2).apply(&mut layout);
            let empty = selected(&layout, 2).0;
            let populated = selected(&layout, 1).0;
            layout.swap_workspace_with_monitor(true);
            assert_eq!(selected(&layout, 1).0, empty);
            assert_eq!(selected(&layout, 2).0, populated);
            assert!(layout.has_window(&1));
            layout.verify_invariants();
            layout.swap_workspace_with_monitor(false);
            layout.verify_invariants();
        }
    }
}

#[test]
fn workspace_set_rotation_preserves_hidden_desks_pins_selection_and_collection_identity() {
    let mut layout = layout(true, false);
    populate(&mut layout, 3);
    layout.toggle_window_pinned(Some(&1));
    // Leave a populated hidden collection, independent of the shown ring.
    let hidden = layout.create_collection(Some("hidden".into()));
    Op::AddWindow { params: TestWindowParams::new(4) }.apply(&mut layout);
    layout.focus_collection_previous();
    Op::CompleteAnimations.apply(&mut layout);
    let hidden_stack: Vec<_> = layout.collection_workspaces(hidden).map(Workspace::id).collect();
    let before: Vec<_> = (1..=3).map(|id| (monitor(&layout, id).collection_id, selected(&layout, id).0)).collect();
    let homes: Vec<_> = layout.collections.iter().map(|c| (c.id, c.original_output.clone())).collect();
    layout.rotate_workspace_sets(true);
    for id in 1..=3 {
        assert_eq!(monitor(&layout, id).collection_id, before[id - 1].0);
        assert_eq!(selected(&layout, id).0, before[(id + 1) % 3].1);
    }
    assert!(monitor(&layout, 1).pinned_workspace.has_window(&1));
    assert_eq!(layout.active_output().unwrap().name(), "output1");
    assert_eq!(layout.collection_workspaces(hidden).map(Workspace::id).collect::<Vec<_>>(), hidden_stack);
    assert_eq!(layout.collections.iter().map(|c| (c.id, c.original_output.clone())).collect::<Vec<_>>(), homes);
    layout.verify_invariants();
    layout.rotate_workspace_sets(false);
    assert_eq!((1..=3).map(|id| (monitor(&layout, id).collection_id, selected(&layout, id).0)).collect::<Vec<_>>(), before);
    layout.verify_invariants();
}

#[test]
fn physical_ring_uses_coordinates_then_name_and_wraps() {
    let mut layout = layout(true, false);
    populate(&mut layout, 3);
    // Ring: output3, output1, output2 (output1/output2 tie broken by name).
    monitor(&layout, 3).output.change_current_state(None, None, None, Some((-100, 0).into()));
    assert_eq!(layout.physical_monitor_ring(), vec![2, 0, 1]);
    Op::FocusOutput(2).apply(&mut layout);
    let before = selected(&layout, 3).0;
    layout.swap_workspace_with_monitor(true);
    assert_eq!(selected(&layout, 2).0, before);
    layout.verify_invariants();
}

#[test]
fn monitor_actions_survive_hotplug_zero_outputs_and_disabled_collections() {
    for collections in [false, true] {
        let mut layout = layout(collections, true);
        layout.swap_workspace_with_monitor(true);
        layout.rotate_workspace_sets(false);
        populate(&mut layout, 3);
        let before: Vec<_> = (1..=3).map(|id| selected(&layout, id).0).collect();
        layout.rotate_workspace_sets(true);
        if !collections {
            assert_eq!((1..=3).map(|id| selected(&layout, id).0).collect::<Vec<_>>(), before);
        }
        layout.swap_workspace_with_monitor(false);
        for id in [2, 3, 1] {
            Op::RemoveOutput(id).apply(&mut layout);
            layout.swap_workspace_with_monitor(true);
            layout.rotate_workspace_sets(false);
            layout.verify_invariants();
        }
        assert_eq!(layout.outputs().count(), 0);
        for id in [3, 2, 1] {
            Op::AddOutput(id).apply(&mut layout);
            layout.verify_invariants();
        }
        for id in 1..=3 {
            assert!(layout.has_window(&id));
        }
    }
}

#[test]
fn monitor_exchange_keeps_unequal_indices_and_rebinds_different_output_scales() {
    let mut layout = layout(true, false);
    populate(&mut layout, 1);
    layout.switch_workspace_down();
    Op::AddWindow { params: TestWindowParams::new(2) }.apply(&mut layout);
    Op::AddScaledOutput { id: 2, scale: 1.5, layout_config: None }.apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    Op::AddWindow { params: TestWindowParams::new(3) }.apply(&mut layout);
    Op::FocusOutput(1).apply(&mut layout);
    Op::CompleteAnimations.apply(&mut layout);
    let a = selected(&layout, 1);
    let b = selected(&layout, 2);
    assert_ne!(a.1, b.1);
    layout.swap_workspace_with_monitor(true);
    assert_eq!(selected(&layout, 1), (b.0, a.1));
    assert_eq!(selected(&layout, 2), (a.0, b.1));
    layout.verify_invariants();
    layout.rotate_workspace_sets(true);
    layout.verify_invariants();
    layout.rotate_workspace_sets(false);
    layout.swap_workspace_with_monitor(false);
    assert_eq!(selected(&layout, 1), a);
    assert_eq!(selected(&layout, 2), b);
    layout.verify_invariants();
}

#[test]
fn explicit_workspace_set_moves_survive_reload_and_destination_hotplug() {
    for collections in [false, true] {
        for rotate in [false, true] {
            // Set rotation requires collections; selected exchange also supports upstream mode.
            if rotate && !collections {
                continue;
            }
            let mut layout = layout(collections, false);
            populate(&mut layout, 2);
            for id in 1..=2 {
                let m = layout.monitors_mut().find(|m| m.output_name() == &format!("output{id}")).unwrap();
                m.workspaces[m.active_workspace_idx].name = Some(format!("set{id}"));
            }
            if rotate {
                layout.rotate_workspace_sets(true);
            } else {
                layout.swap_workspace_with_monitor(true);
            }
            // Model Nacre's saved declarations and subsequent config reload: both named sets
            // are declared at their new destinations. Existing workspace identities survive.
            let mut config = Config::default();
            for (name, destination) in [("set1", "output2"), ("set2", "output1")] {
                let workspace = WorkspaceConfig {
                    name: WorkspaceName(name.into()),
                    open_on_output: Some(destination.into()),
                    layout: None,
                };
                layout.ensure_named_workspace(&workspace);
                config.workspaces.push(workspace);
            }
            config.layout.collections.enabled = collections;
            layout.update_config(&config);
            let destination = monitor(&layout, 2).output.clone();
            layout.remove_output(&destination);
            layout.verify_invariants();
            layout.add_output(destination, None);
            for (id, name) in [(1, "set2"), (2, "set1")] {
                assert!(monitor(&layout, id).workspaces.iter().any(|workspace| workspace.name().is_some_and(|candidate| candidate == name)));
            }
            assert!(layout.has_window(&1));
            assert!(layout.has_window(&2));
            layout.verify_invariants();
        }
    }
}
