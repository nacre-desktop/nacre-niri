//! Regression coverage for collection ownership across config and hidden-window changes.

use super::*;

fn collections_layout() -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.collections.enabled = true;
    Layout::with_options(Clock::with_time(Duration::ZERO), options)
}

fn add_window(layout: &mut Layout<TestWindow>, id: usize) {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
    .apply(layout);
    layout.verify_invariants();
}

fn output(layout: &Layout<TestWindow>, number: usize) -> Output {
    let name = format!("output{number}");
    layout
        .outputs()
        .find(|output| output.name() == name)
        .unwrap()
        .clone()
}

#[test]
fn configured_empty_collection_survives_close_and_reload_until_unconfigured() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    config.layout.collections.names = vec!["work".into()];
    let mut layout = Layout::<TestWindow>::new(Clock::with_time(Duration::ZERO), &config);
    let work = layout.collections()[0].id;
    assert_eq!(layout.collection_workspaces(work).count(), 1);
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    add_window(&mut layout, 1);
    layout.create_collection(Some("scratch".into()));
    layout.remove_window(&1, Transaction::new()).unwrap();
    layout.refresh(true);
    layout.verify_invariants();
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == work));

    config.layout.collections.names.push("extra".into());
    layout.update_config(&config);
    layout.refresh(true);
    layout.verify_invariants();
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == work));
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.name.as_deref() == Some("extra")));

    config.layout.collections.names.clear();
    layout.update_config(&config);
    layout.refresh(true);
    layout.verify_invariants();
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.id == work));
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.name.as_deref() == Some("extra")));
}

#[test]
fn renamed_runtime_collection_survives_becoming_empty_while_hidden() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    let home = layout.collection_for_output(&output(&layout, 1)).unwrap();
    let named = layout.create_collection(None);
    assert!(layout.set_collection_name(&WorkspaceReference::Id(named), Some("research".into())));
    add_window(&mut layout, 10);
    layout.focus_collection(&WorkspaceReference::Id(home));
    layout.remove_window(&10, Transaction::new()).unwrap();
    layout.refresh(true);
    layout.verify_invariants();
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == named));
    assert!(layout
        .collection_workspaces(named)
        .all(|workspace| !workspace.has_windows()));

    let unnamed = layout.create_collection(None);
    add_window(&mut layout, 11);
    layout.focus_collection(&WorkspaceReference::Id(home));
    layout.remove_window(&11, Transaction::new()).unwrap();
    layout.refresh(true);
    layout.verify_invariants();
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.id == unnamed));
}

#[test]
fn toggling_collections_restores_original_output_ownership_after_swap() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    add_window(&mut layout, 1);
    check_ops_on_layout(&mut layout, [Op::AddOutput(2), Op::FocusOutput(2)]);
    add_window(&mut layout, 2);
    let first = output(&layout, 1);
    let second = output(&layout, 2);
    let second_collection = layout.collection_for_output(&second).unwrap();
    layout.focus_output(&first);
    layout.focus_collection(&WorkspaceReference::Id(second_collection));
    layout.verify_invariants();
    assert!(layout
        .windows_for_output(&first)
        .any(|window| *window.id() == 2));
    assert!(layout
        .windows_for_output(&second)
        .any(|window| *window.id() == 1));

    let mut config = Config::default();
    config.layout.collections.enabled = false;
    layout.update_config(&config);
    layout.verify_invariants();
    assert!(layout.collections().is_empty());
    assert!(layout
        .windows_for_output(&first)
        .any(|window| *window.id() == 1));
    assert!(layout
        .windows_for_output(&second)
        .any(|window| *window.id() == 2));

    config.layout.collections.enabled = true;
    layout.update_config(&config);
    layout.verify_invariants();
    assert_eq!(layout.collections().len(), 2);
    assert!(layout.collection_for_output(&first).is_some());
    assert!(layout.collection_for_output(&second).is_some());
    assert_eq!(layout.windows().count(), 2);
}

#[test]
fn new_output_reuses_most_recent_hidden_collection_before_creating_one() {
    let mut layout = collections_layout();
    check_ops_on_layout(
        &mut layout,
        [Op::AddOutput(1), Op::AddOutput(2), Op::AddOutput(3)],
    );
    assert_eq!(
        layout.collections().len(),
        3,
        "a new empty desk is created when no hidden desk exists"
    );
    let active_output = layout.active_output().unwrap().clone();
    let home = layout.collection_for_output(&active_output).unwrap();
    let created = layout.create_collection(Some("recent".into()));
    layout.focus_collection(&WorkspaceReference::Id(home));
    let original_output = layout
        .collections
        .iter()
        .find(|collection| collection.id == created)
        .unwrap()
        .original_output
        .clone();
    let count = layout.collections().len();
    check_ops_on_layout(&mut layout, [Op::AddOutput(4)]);
    assert_eq!(layout.collections().len(), count);
    assert_eq!(
        layout.collection_for_output(&output(&layout, 4)),
        Some(created)
    );
    assert_eq!(
        layout
            .collections
            .iter()
            .find(|collection| collection.id == created)
            .unwrap()
            .original_output,
        original_output
    );
    layout.verify_invariants();
}

#[test]
fn window_focus_updates_hidden_desk_mru_for_later_hotplug() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1), Op::AddOutput(2)]);
    let output2 = output(&layout, 2);
    let recent = layout.create_collection(Some("recent-focus".into()));
    add_window(&mut layout, 90);

    layout.focus_output(&output2);
    layout.activate_window(&90);
    layout.refresh(true);
    assert_eq!(layout.collection_recent_use.last(), Some(&recent));

    check_ops_on_layout(&mut layout, [Op::RemoveOutput(1)]);
    check_ops_on_layout(&mut layout, [Op::AddOutput(3)]);
    assert_eq!(
        layout.collection_for_output(&output(&layout, 3)),
        Some(recent)
    );
    assert_eq!(
        layout
            .collections
            .iter()
            .find(|collection| collection.id == recent)
            .unwrap()
            .original_output,
        None
    );
    assert_ne!(layout.collection_for_output(&output2), Some(recent));
    layout.verify_invariants();
}

#[test]
fn toggling_collections_with_all_outputs_removed_keeps_windows() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    add_window(&mut layout, 1);
    check_ops_on_layout(&mut layout, [Op::AddOutput(2), Op::FocusOutput(2)]);
    add_window(&mut layout, 2);
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(2), Op::RemoveOutput(1)]);
    assert_eq!(layout.windows().count(), 2);

    let mut config = Config::default();
    config.layout.collections.enabled = false;
    layout.update_config(&config);
    layout.verify_invariants();
    assert!(layout.collections().is_empty());
    assert_eq!(layout.windows().count(), 2);
    config.layout.collections.enabled = true;
    layout.update_config(&config);
    layout.verify_invariants();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    assert_eq!(layout.windows().count(), 2);
    assert!(layout.has_window(&1));
    assert!(layout.has_window(&2));
}

#[test]
fn explicit_hidden_window_moves_between_workspaces_and_onto_output() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    add_window(&mut layout, 1);
    let first = output(&layout, 1);
    let hidden = layout.collection_for_output(&first).unwrap();
    let shown = layout.create_collection(Some("scratch".into()));
    layout.move_to_workspace(Some(&1), 1, ActivateWindow::Yes);
    layout.verify_invariants();
    assert!(layout
        .collection_workspaces(hidden)
        .nth(1)
        .unwrap()
        .has_window(&1));
    assert_eq!(layout.collection_for_output(&first), Some(shown));

    layout.move_to_output(Some(&1), &first, None, ActivateWindow::No);
    layout.verify_invariants();
    assert!(layout
        .windows_for_output(&first)
        .any(|window| *window.id() == 1));
    // The connected monitor's original empty desk remains available for rotation.
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == hidden));
    assert!(layout
        .collection_workspaces(hidden)
        .all(|workspace| !workspace.has_windows()));
    assert_eq!(layout.collection_for_output(&first), Some(shown));
    assert_eq!(layout.windows().count(), 1);
}

#[test]
fn new_window_can_target_a_hidden_collection_without_showing_it() {
    let mut layout = collections_layout();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    add_window(&mut layout, 1);
    let first = output(&layout, 1);
    let hidden = layout.collection_for_output(&first).unwrap();
    let workspace = layout
        .collection_workspaces(hidden)
        .find(|workspace| workspace.has_window(&1))
        .unwrap()
        .id();
    let shown = layout.create_collection(Some("scratch".into()));
    assert!(layout
        .add_window(
            TestWindow::new(TestWindowParams::new(2)),
            AddWindowTarget::Workspace(workspace),
            None,
            None,
            false,
            false,
            ActivateWindow::Yes,
        )
        .is_none());
    layout.refresh(true);
    layout.verify_invariants();
    assert_eq!(layout.collection_for_output(&first), Some(shown));
    assert!(layout
        .collection_workspaces(hidden)
        .any(|workspace| workspace.has_window(&2)));
    let (monitor, window) = layout
        .windows()
        .find(|(_, window)| *window.id() == 2)
        .unwrap();
    assert!(monitor.is_none());
    assert!(!window.0.pending_activated.get());
}

#[test]
fn window_opened_before_first_output_gets_a_trailing_empty_workspace() {
    let mut layout = collections_layout();
    add_window(&mut layout, 1);
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);
    let monitor = layout.active_monitor_ref().unwrap();
    assert_eq!(monitor.active_window().map(|window| *window.id()), Some(1));
    assert!(!monitor.workspaces.last().unwrap().has_windows_or_name());
    assert_eq!(layout.windows().count(), 1);
}

#[test]
fn configured_workspaces_route_to_their_first_outputs_without_splitting_on_replug() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    config.workspaces = [1, 2]
        .into_iter()
        .map(|number| WorkspaceConfig {
            name: WorkspaceName(format!("workspace{number}")),
            open_on_output: Some(format!("output{number}")),
            layout: None,
        })
        .collect();
    let mut layout = Layout::<TestWindow>::new(Clock::with_time(Duration::ZERO), &config);
    check_ops_on_layout(&mut layout, [Op::AddOutput(1), Op::AddOutput(2)]);
    let first = output(&layout, 1);
    let second = output(&layout, 2);
    assert_eq!(
        layout.monitor_for_workspace("workspace1").unwrap().output(),
        &first
    );
    assert_eq!(
        layout.monitor_for_workspace("workspace2").unwrap().output(),
        &second
    );
    layout.focus_output(&second);
    add_window(&mut layout, 2);
    let collection = layout.collection_for_output(&second).unwrap();
    let workspaces = layout
        .collections()
        .into_iter()
        .find(|candidate| candidate.id == collection)
        .unwrap()
        .workspace_ids;
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(2), Op::AddOutput(2)]);
    let restored = output(&layout, 2);
    assert_eq!(layout.collection_for_output(&restored), Some(collection));
    assert_eq!(
        layout
            .collections()
            .into_iter()
            .find(|candidate| candidate.id == collection)
            .unwrap()
            .workspace_ids,
        workspaces
    );
    assert!(layout
        .windows_for_output(&restored)
        .any(|window| *window.id() == 2));
}
