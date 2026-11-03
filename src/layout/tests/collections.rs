use super::*;

fn enabled_layout() -> Layout<TestWindow> {
    Layout::with_options(Clock::with_time(Duration::ZERO), Options::default())
}

fn shown(layout: &Layout<TestWindow>, output: &str) -> u64 {
    layout
        .collections()
        .iter()
        .find(|c| c.output.as_deref() == Some(output))
        .unwrap()
        .id
}

fn add_window(layout: &mut Layout<TestWindow>, id: usize) {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
    .apply(layout);
}

#[test]
fn unplug_keeps_collection_detached() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let laptop = shown(&layout, "output1");
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    let external = shown(&layout, "output2");
    let workspace_ids = layout
        .collections()
        .into_iter()
        .find(|c| c.id == external)
        .unwrap()
        .workspace_ids;
    Op::RemoveOutput(2).apply(&mut layout);
    let collections = layout.collections();
    let detached = collections.iter().find(|c| c.id == external).unwrap();
    assert!(detached.output.is_none());
    assert_eq!(detached.workspace_ids, workspace_ids);
    let laptop = collections.iter().find(|c| c.id == laptop).unwrap();
    assert!(laptop
        .workspace_ids
        .iter()
        .all(|id| !workspace_ids.contains(id)));
    assert!(layout.has_window(&1));
    layout.verify_invariants();
}

#[test]
fn replug_restores_collection_and_laptop_previous() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let laptop = shown(&layout, "output1");
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 2);
    let external = shown(&layout, "output2");
    Op::RemoveOutput(2).apply(&mut layout);
    layout.focus_collection(&WorkspaceReference::Id(external));
    assert_eq!(shown(&layout, "output1"), external);
    Op::AddOutput(2).apply(&mut layout);
    assert_eq!(shown(&layout, "output2"), external);
    assert_eq!(shown(&layout, "output1"), laptop);
    layout.verify_invariants();
}

#[test]
fn replug_after_connected_swap_restores_both_home_collections() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let laptop = shown(&layout, "output1");
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 2);
    let external = shown(&layout, "output2");
    Op::FocusOutput(1).apply(&mut layout);
    layout.focus_collection(&WorkspaceReference::Id(external));
    assert_eq!(shown(&layout, "output2"), laptop);
    Op::RemoveOutput(2).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    assert_eq!(shown(&layout, "output1"), laptop);
    assert_eq!(shown(&layout, "output2"), external);
    assert_eq!(layout.collections().len(), 2);
    layout.verify_invariants();
}

#[test]
fn replug_after_focusing_current_borrowed_collection_does_not_duplicate_it() {
    let mut layout = enabled_layout();
    Op::AddOutput(5).apply(&mut layout);
    let output5_home = shown(&layout, "output5");
    Op::AddOutput(1).apply(&mut layout);
    let output1_home = shown(&layout, "output1");
    check_ops_on_layout(
        &mut layout,
        [
            Op::CycleCollection(false),
            Op::RemoveOutput(5),
            Op::FocusCollection(1),
            Op::AddScaledOutput {
                id: 5,
                scale: 1.0,
                layout_config: None,
            },
        ],
    );
    assert_eq!(shown(&layout, "output5"), output5_home);
    assert_eq!(shown(&layout, "output1"), output1_home);
    layout.verify_invariants();
}

#[test]
fn replug_does_not_swap_away_for_a_return_collection_already_shown_elsewhere() {
    let mut layout = enabled_layout();
    Op::AddOutput(5).apply(&mut layout);
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);

    let returning = shown(&layout, "output5");
    let already_shown = shown(&layout, "output2");
    Op::FocusOutput(1).apply(&mut layout);
    layout.focus_collection(&WorkspaceReference::Id(returning));
    assert_eq!(shown(&layout, "output1"), returning);
    Op::RemoveOutput(5).apply(&mut layout);

    // A remembered return can become stale when its collection is shown on another output.
    layout
        .collection_returns
        .insert("output1".to_owned(), already_shown);

    Op::AddScaledOutput {
        id: 5,
        scale: 1.0,
        layout_config: None,
    }
    .apply(&mut layout);

    layout.verify_invariants();
    assert_eq!(shown(&layout, "output5"), returning);
    assert_eq!(shown(&layout, "output2"), already_shown);
    assert_ne!(shown(&layout, "output1"), returning);
}

#[test]
fn emptied_detached_collection_is_removed() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    let external = shown(&layout, "output2");
    Op::RemoveOutput(2).apply(&mut layout);
    Op::CloseWindow(1).apply(&mut layout);
    layout.refresh(true);
    assert!(!layout.has_window(&1));
    assert!(!layout.collections().iter().any(|c| c.id == external));
    layout.verify_invariants();
}

#[test]
fn laptop_virtual_collection_rotates() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("desk".to_owned()));
    let second = shown(&layout, "output1");
    assert_ne!(first, second);
    add_window(&mut layout, 2);
    layout.focus_collection_previous();
    assert_eq!(shown(&layout, "output1"), first);
    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    layout.verify_invariants();
}

#[test]
fn empty_laptop_home_remains_available_and_empty_virtual_desk_expires() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let home = shown(&layout, "output1");
    let desk = layout.create_collection(None);
    assert_ne!(home, desk);
    layout.focus_collection_previous();
    assert_eq!(shown(&layout, "output1"), home);
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.id == desk));
    layout.verify_invariants();
}

#[test]
fn move_window_between_collections_preserves_window() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("desk".to_owned()));
    add_window(&mut layout, 2);
    layout.move_window_to_collection(&WorkspaceReference::Id(first));
    assert!(layout.has_window(&2));
    layout.focus_collection(&WorkspaceReference::Id(first));
    assert!(layout.active_workspace().unwrap().has_window(&2));
    layout.verify_invariants();
}

#[test]
fn overview_hit_testing_selects_detached_column() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("other".to_owned()));
    add_window(&mut layout, 2);
    let other = shown(&layout, "output1");
    let target = layout.collection_active_workspace(other).unwrap();
    layout.focus_collection(&WorkspaceReference::Id(first));
    layout.open_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);
    let output = layout.active_output().unwrap().clone();
    // Hidden previews participate in presentation feedback and frame callbacks, while their
    // actual workspace output remains detached.
    assert_eq!(layout.windows_for_output(&output).count(), 2);
    assert_eq!(layout.windows_for_output_mut(&output).count(), 2);
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .current_output()
        .is_none());
    let columns = super::super::collection_overview::columns(
        layout.collections().iter().map(|c| c.id),
        first,
        (1280., 720.).into(),
        layout.overview_zoom(),
    );
    let column = columns.iter().find(|c| c.id == other).unwrap();
    let point = (column.bounds.loc.x + column.bounds.size.w / 2., 360.).into();
    let focused = *layout.focus().unwrap().id();
    let camera = layout.active_monitor().unwrap().workspace_render_idx();
    // The workspace center may be empty: test windows keep a small client buffer until
    // their first configure acknowledgement. Aim inside the actual rendered tile.
    let (workspace, geometry, zoom) = layout.collection_window_preview(&output, &2).unwrap();
    let (tile, position, _) = workspace
        .tiles_with_render_positions()
        .find(|(tile, _, visible)| *visible && tile.window().id() == &2)
        .unwrap();
    let hover = geometry.loc + (position + tile.tile_size().to_point().downscale(2.)).upscale(zoom);
    assert_eq!(layout.window_under(&output, hover).unwrap().0.id(), &2);
    assert!(!layout.activate_window_without_scrolling(&2));
    assert_eq!(layout.focus().unwrap().id(), &focused);
    assert_eq!(
        layout.active_monitor().unwrap().workspace_render_idx(),
        camera
    );
    let workspace = layout.workspace_under(false, &output, point).unwrap().id();
    assert_eq!(workspace, target);
    layout.select_overview_workspace(&output, workspace);
    assert_eq!(shown(&layout, "output1"), other);
    assert!(!layout.is_overview_open());
    layout.verify_invariants();
}

#[test]
fn overview_hover_selects_visible_workspace_without_moving_camera() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    Op::FocusWorkspaceDown.apply(&mut layout);
    add_window(&mut layout, 2);
    Op::FocusWorkspaceUp.apply(&mut layout);
    layout.open_overview();
    let camera = layout.monitors().next().unwrap().workspace_render_idx();

    assert!(layout.activate_window_without_scrolling(&2));
    layout.refresh(true);
    assert_eq!(layout.focus().unwrap().id(), &2);
    assert_eq!(
        layout.monitors().next().unwrap().workspace_render_idx(),
        camera
    );
    layout.verify_invariants();
    layout.close_overview();
    layout.verify_invariants();
}

#[test]
fn overview_hover_works_when_collections_are_disabled() {
    let mut options = Options::default();
    options.layout.collections.enabled = false;
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    Op::FocusWorkspaceDown.apply(&mut layout);
    add_window(&mut layout, 2);
    Op::FocusWorkspaceUp.apply(&mut layout);
    layout.open_overview();
    let camera = layout.monitors().next().unwrap().workspace_render_idx();

    assert!(layout.activate_window_without_scrolling(&2));
    layout.refresh(true);
    assert_eq!(layout.focus().unwrap().id(), &2);
    assert_eq!(
        layout.monitors().next().unwrap().workspace_render_idx(),
        camera
    );
    layout.verify_invariants();
}

#[test]
fn disabling_overview_hover_clears_hover_camera_state() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    Op::FocusWorkspaceDown.apply(&mut layout);
    add_window(&mut layout, 2);
    Op::FocusWorkspaceUp.apply(&mut layout);
    layout.open_overview();
    assert!(layout.activate_window_without_scrolling(&2));
    assert!(matches!(
        layout.monitors().next().unwrap().workspace_switch.as_ref(),
        Some(super::super::monitor::WorkspaceSwitch::Hover(_))
    ));

    layout.update_options(Options {
        nacre: niri_config::Nacre {
            overview_hover: false,
            ..Default::default()
        },
        ..Default::default()
    });

    assert!(!layout.activate_window_without_scrolling(&1));
    assert!(!matches!(
        layout.monitors().next().unwrap().workspace_switch.as_ref(),
        Some(super::super::monitor::WorkspaceSwitch::Hover(_))
    ));
    layout.verify_invariants();
}

#[test]
fn overview_drag_moves_window_into_hidden_collection() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("other".to_owned()));
    add_window(&mut layout, 2);
    let other = shown(&layout, "output1");
    let target = layout.collection_active_workspace(other).unwrap();
    layout.focus_collection(&WorkspaceReference::Id(first));
    layout.open_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);
    let output = layout.active_output().unwrap().clone();
    let columns = super::super::collection_overview::columns(
        layout.collections().iter().map(|c| c.id),
        first,
        (1280., 720.).into(),
        layout.overview_zoom(),
    );
    let source = columns.iter().find(|c| c.id == first).unwrap();
    let destination = columns.iter().find(|c| c.id == other).unwrap();
    let from = (source.bounds.loc.x + 40., 360.).into();
    let to = (destination.bounds.loc.x + 40., 360.).into();
    assert!(layout.interactive_move_begin(1, &output, from));
    assert!(layout.interactive_move_update(&1, (300., 0.).into(), output, to));
    layout.interactive_move_end(&1);
    assert!(layout
        .find_workspace_by_id(target)
        .unwrap()
        .1
        .has_window(&1));
    assert_eq!(shown(&layout, "output1"), first);
    layout.verify_invariants();
}

#[test]
fn operation_generator_does_not_duplicate_detached_window_ids() {
    let mut layout = enabled_layout();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(5),
            },
            Op::RemoveOutput(1),
            Op::AddNamedWorkspace {
                ws_name: 1,
                output_name: None,
                layout_config: None,
            },
            Op::AddWindowToNamedWorkspace {
                params: TestWindowParams::new(5),
                ws_name: 1,
            },
        ],
    );
    assert_eq!(layout.windows().count(), 1);
}

#[test]
fn naming_workspace_by_id_updates_its_owning_monitor_boundaries() {
    let mut options = Options::default();
    options.layout.collections.names = vec!["collection2".into(), "collection3".into()];
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::CreateCollection(Some(9)),
            Op::CreateCollection(Some(3)),
            Op::AddOutput(2),
            Op::RemoveOutput(1),
        ],
    );
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindowPrevious,
            Op::AddOutput(3),
            Op::MoveWindowToWorkspace {
                window_id: None,
                workspace_idx: 1,
            },
            Op::SetWorkspaceNameByReference {
                new_ws_name: 1,
                reference: WorkspaceReferenceOp::ExistingId(3),
            },
        ],
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: ProptestConfig::default().cases.max(128),
        ..ProptestConfig::default()
    })]
    #[test]
    fn collection_operations_preserve_invariants(ops in prop::collection::vec(any::<Op>(), 0..100)) {
        let mut options = Options::default();
        options.layout.collections.names = vec!["collection2".into(), "collection3".into()];
        let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
        // Start with a visible output, a hidden populated collection, and a persistent empty
        // collection. Unplugging the first output detaches both collections while the second
        // output remains active, so random operations start in the mixed state that exposed MRU.
        check_ops_on_layout(
            &mut layout,
            [
                Op::AddOutput(1),
                Op::AddWindow { params: TestWindowParams::new(1) },
                Op::CreateCollection(Some(9)),
                Op::CreateCollection(Some(3)),
                Op::AddOutput(2),
                Op::RemoveOutput(1),
            ],
        );
        assert!(layout.active_output().is_some());
        let detached: Vec<_> = layout
            .collections()
            .into_iter()
            .filter(|collection| collection.output.is_none())
            .collect();
        assert!(detached.iter().any(|collection| {
            layout
                .collection_workspaces(collection.id)
                .any(|workspace| workspace.has_windows())
        }));
        assert!(detached.iter().any(|collection| {
            layout
                .collection_workspaces(collection.id)
                .any(|workspace| !workspace.has_windows())
        }));
        check_ops_on_layout(&mut layout, ops);
    }
}

#[test]
fn collection_rename_persists_empty_named_collection_and_rejects_duplicates() {
    let mut options = Options::default();
    options.layout.collections.enabled = true;
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    Op::AddOutput(1).apply(&mut layout);
    let created = layout.create_collection(Some("  research  ".into()));
    assert_eq!(
        layout.collections().iter().find(|c| c.id == created).unwrap().name.as_deref(),
        Some("research")
    );
    assert!(layout.set_collection_name(&WorkspaceReference::Id(created), Some("work".into())));
    assert!(!layout.set_collection_name(&WorkspaceReference::Index(1), Some("work".into())));
    assert!(layout.set_collection_name(&WorkspaceReference::Id(created), Some("  ".into())));
    assert!(layout.collections().iter().any(|c| c.id == created));
    layout.verify_invariants();
}
