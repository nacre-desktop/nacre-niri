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
fn collection_swipe_tracks_fraction_of_output_width_and_cancel_keeps_source() {
    for width in [1280, 1920] {
        let mut layout = enabled_layout();
        Op::AddOutput(1).apply(&mut layout);
        let output = layout.outputs().next().unwrap().clone();
        output.change_current_state(
            Some(Mode {
                size: Size::from((width, 720)),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        layout.update_output_size(&output);
        let source = shown(&layout, "output1");
        let target = layout.create_collection(Some("desk".into()));
        assert_ne!(source, target);

        assert!(layout.collection_switch_gesture_begin(&output));
        assert_eq!(
            layout.collection_switch_gesture_update(-150., Duration::from_millis(10)),
            Some(output.clone())
        );
        assert!(
            (layout
                .monitor_for_output(&output)
                .unwrap()
                .collection_switch_offset_x
                + f64::from(width) / 2.)
                .abs()
                < 0.01
        );
        let (_, incoming_x) = layout
            .collection_switch_render_workspace(&output)
            .expect("adjacent desk is rendered during the gesture");
        assert!((incoming_x - f64::from(width) / 2.).abs() < 0.01);

        layout.collection_switch_gesture_end(true);
        assert_eq!(shown(&layout, "output1"), target);
        assert!(layout.collection_switch_render_workspace(&output).is_some());
        layout.verify_invariants();
    }
}

#[test]
fn first_collection_swipe_frame_uses_only_the_axis_recognition_event() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let output = layout.outputs().next().unwrap().clone();
    layout.create_collection(Some("desk".into()));
    let width = layout.monitor_for_output(&output).unwrap().view_size().w;

    assert!(layout.collection_switch_gesture_begin(&output));
    layout.collection_switch_gesture_update(-4., Duration::from_millis(1));
    let offset = layout
        .monitor_for_output(&output)
        .unwrap()
        .collection_switch_offset_x;
    assert!((offset + width * 4. / WORKSPACE_GESTURE_MOVEMENT).abs() < 0.01);
}

#[test]
fn collection_swipe_to_a_collection_shown_elsewhere_leases_only_on_commit() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    let output1 = layout
        .outputs()
        .find(|output| output.name() == "output1")
        .unwrap()
        .clone();
    let source = shown(&layout, "output1");
    let target = shown(&layout, "output2");

    assert!(layout.collection_switch_gesture_begin(&output1));
    layout.collection_switch_gesture_update(-180., Duration::from_millis(10));
    assert_eq!(shown(&layout, "output1"), source);
    assert_eq!(shown(&layout, "output2"), target);
    layout.collection_switch_gesture_end(true);
    assert_eq!(shown(&layout, "output1"), source);
    assert_eq!(shown(&layout, "output2"), target);

    assert!(layout.collection_switch_gesture_begin(&output1));
    layout.collection_switch_gesture_update(-180., Duration::from_millis(20));
    layout.collection_switch_gesture_end(false);
    assert_eq!(shown(&layout, "output1"), target);
    assert_eq!(shown(&layout, "output2"), source);
    layout.verify_invariants();
}

#[test]
fn incoming_collection_fit_transform_matches_different_output_size_and_scale() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    let output1 = layout
        .outputs()
        .find(|output| output.name() == "output1")
        .unwrap()
        .clone();
    let output2 = layout
        .outputs()
        .find(|output| output.name() == "output2")
        .unwrap()
        .clone();
    output1.change_current_state(
        Some(Mode {
            size: Size::from((600, 450)),
            refresh: 60_000,
        }),
        None,
        Some(smithay::output::Scale::Fractional(1.5)),
        None,
    );
    output2.change_current_state(
        Some(Mode {
            size: Size::from((900, 600)),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    layout.update_output_size(&output1);
    layout.update_output_size(&output2);
    let outgoing_id = layout
        .monitor_for_output(&output1)
        .unwrap()
        .active_workspace_ref()
        .id()
        .get();
    assert!(layout.collection_switch_gesture_begin(&output1));
    layout.collection_switch_gesture_update(-180., Duration::from_millis(10));
    let (incoming, incoming_x) = layout
        .collection_switch_render_workspace(&output1)
        .expect("desk from the other output is available to preview");
    assert_eq!(incoming.view_size(), Size::from((900., 600.)));
    assert_eq!(incoming_x, 160.);
    let expected = (0.5, Point::from((135., 0.)));
    assert_eq!(
        layout.collection_switch_transform_for_workspace(&output1, incoming.id().get()),
        Some(expected)
    );
    let outgoing = layout
        .collection_switch_transform_for_workspace(&output1, outgoing_id)
        .expect("outgoing desk retains its swipe translation");
    assert_eq!(outgoing.0, 1.);
    assert_eq!(outgoing.1.x, -240.);
    assert!(layout.collection_switch_involves_workspace(&output1, outgoing_id));
    assert!(layout.collection_switch_involves_workspace(&output1, incoming.id().get()));
}

#[test]
fn collection_swipe_retarget_and_hotplug_clear_transient_state() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let output = layout.outputs().next().unwrap().clone();
    layout.create_collection(Some("desk".into()));

    assert!(layout.collection_switch_gesture_begin(&output));
    layout.collection_switch_gesture_update(-180., Duration::ZERO);
    layout.collection_switch_gesture_end(false);
    assert!(layout.collection_switch_render_workspace(&output).is_some());

    // Starting a new gesture catches the in-flight settling animation and keeps the other desk
    // available for drawing while the new finger delta retargets the transition.
    assert!(layout.collection_switch_gesture_begin(&output));
    layout.collection_switch_gesture_update(30., Duration::from_millis(10));
    assert!(layout.collection_switch_render_workspace(&output).is_some());

    layout.remove_output(&output);
    assert!(layout.collection_switch_render_workspace(&output).is_none());
    assert!(layout
        .monitors()
        .all(|monitor| !monitor.collection_switch_active));
    layout.verify_invariants();
}

#[test]
fn one_collection_does_not_start_a_horizontal_transition() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let output = layout.outputs().next().unwrap().clone();
    assert!(!layout.collection_switch_gesture_available(&output));
    assert!(!layout.collection_switch_gesture_begin(&output));
}

#[test]
fn unname_and_disable_clear_persisted_workspace_retention() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let mut layout = Layout::new(Clock::with_time(Duration::ZERO), &config);
    Op::AddOutput(1).apply(&mut layout);
    let output = layout.outputs().next().unwrap().clone();
    let first = layout.create_collection(Some("desk".into()));
    let workspace_id = layout.collection_active_workspace(first).unwrap();
    layout
        .monitor_for_output_mut(&output)
        .unwrap()
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.id() == workspace_id)
        .unwrap()
        .set_collection_persistent(true);

    assert!(layout.set_collection_name(&WorkspaceReference::Id(first), None));
    assert!(!layout
        .monitor_for_output(&output)
        .unwrap()
        .workspaces
        .iter()
        .find(|workspace| workspace.id() == workspace_id)
        .unwrap()
        .collection_persistent());

    let second = layout.create_collection(Some("desk-two".into()));
    let workspace_id = layout.collection_active_workspace(second).unwrap();
    layout
        .monitor_for_output_mut(&output)
        .unwrap()
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.id() == workspace_id)
        .unwrap()
        .set_collection_persistent(true);
    config.layout.collections.enabled = false;
    layout.update_config(&config);
    assert!(layout
        .monitors()
        .flat_map(|monitor| &monitor.workspaces)
        .all(|workspace| !workspace.collection_persistent()));
    layout.verify_invariants();
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
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == desk));
    layout.clock.set_complete_instantly(true);
    layout.update_render_elements(None);
    layout.clock.set_complete_instantly(false);
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.id == desk));
    layout.verify_invariants();
}

#[test]
fn hiding_last_window_keeps_persistent_slot_with_empty_workspace_above_first() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    layout.create_collection(Some("persistent-home".into()));
    add_window(&mut layout, 51);
    let output = layout.outputs().next().unwrap().clone();
    layout
        .monitor_for_output_mut(&output)
        .unwrap()
        .workspaces
        .last_mut()
        .unwrap()
        .set_collection_persistent(true);

    let mut options = Options::clone(&layout.options);
    options.layout.empty_workspace_above_first = true;
    layout.update_options(options);
    let retained = layout.monitor_for_output(&output).unwrap().workspaces[2].id();
    assert_eq!(
        layout.monitor_for_output(&output).unwrap().workspaces.len(),
        3
    );
    layout.clock.set_complete_instantly(true);
    Op::FocusWorkspaceDown.apply(&mut layout);
    layout.advance_animations();
    layout.update_render_elements(None);
    layout.clock.set_complete_instantly(false);
    assert_eq!(
        layout
            .monitor_for_output(&output)
            .unwrap()
            .active_workspace_idx(),
        2
    );

    layout.move_window_to_scratchpad(Some(&51));
    let monitor = layout.monitor_for_output(&output).unwrap();
    assert_eq!(monitor.workspaces.len(), 2);
    assert!(!monitor.workspaces[0].has_windows_or_name());
    assert_eq!(monitor.workspaces[1].id(), retained);
    assert!(monitor.workspaces[1].collection_persistent());
    assert!(!monitor.workspaces[1].has_windows());
    layout.verify_invariants();
}

#[test]
fn restored_persistent_slot_survives_workspace_move_and_scratchpad_hide() {
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
            Op::AddOutput(1),
            Op::UpdateConfig {
                layout_config: Box::new(niri_config::LayoutPart::default()),
            },
            Op::RestoreCollectionState,
            Op::CycleCollection(false),
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::MoveWorkspaceToOutput(2),
            Op::UpdateConfig {
                layout_config: Box::new(niri_config::LayoutPart {
                    empty_workspace_above_first: Some(Flag(true)),
                    ..Default::default()
                }),
            },
            Op::FocusWorkspaceDown,
            Op::CompleteAnimations,
            Op::MoveWindowToScratchpad(Some(4)),
        ],
    );
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
fn overview_drop_hits_hidden_desk_column_with_two_outputs_at_both_scales() {
    for source_scale in [1., 1.5] {
        let mut layout = enabled_layout();
        Op::AddScaledOutput {
            id: 1,
            scale: source_scale,
            layout_config: None,
        }
        .apply(&mut layout);
        Op::AddScaledOutput {
            id: 2,
            scale: if source_scale == 1. { 1.5 } else { 1. },
            layout_config: None,
        }
        .apply(&mut layout);
        let output = layout
            .outputs()
            .find(|output| output.name() == "output1")
            .unwrap()
            .clone();
        add_window(&mut layout, 31);
        let source = layout.collection_for_output(&output).unwrap();
        let target = layout.create_collection(Some("drop-target".into()));
        add_window(&mut layout, 32);
        add_window(&mut layout, 33);
        add_window(&mut layout, 34);
        layout.focus_collection(&WorkspaceReference::Id(source));
        let target_workspace = layout.collection_active_workspace(target).unwrap();
        layout.open_overview();
        layout.clock.set_complete_instantly(true);
        layout.advance_animations();
        layout.clock.set_complete_instantly(false);
        let preview = layout
            .collection_previews(&output)
            .into_iter()
            .find(|preview| preview.id == target)
            .expect("hidden desk preview is present on source output");
        let columns = super::super::collection_overview::columns(
            layout.collections().iter().map(|collection| collection.id),
            source,
            (1280., 720.).into(),
            layout.overview_zoom(),
        );
        let source_column = columns.iter().find(|column| column.id == source).unwrap();
        let from = (source_column.bounds.loc.x + 40., 360.).into();
        let to = preview.geometry.loc + preview.geometry.size.to_point().downscale(2.);
        let target_ws = layout.find_workspace_by_id(target_workspace).unwrap().1;
        let expected = target_ws
            .scrolling_insert_position((to - preview.geometry.loc).downscale(preview.zoom));
        let expected_column = match expected {
            super::super::monitor::InsertPosition::InColumn(column, _)
            | super::super::monitor::InsertPosition::NewColumn(column) => column,
            super::super::monitor::InsertPosition::Floating => panic!("test windows are tiled"),
        };
        assert!(
            target_ws
                .tiles_with_ipc_layouts()
                .filter_map(|(_, layout)| layout.pos_in_scrolling_layout)
                .map(|(column, _)| column)
                .max()
                .unwrap_or(0)
                >= 2
        );
        assert!(layout.interactive_move_begin(31, &output, from));
        assert!(layout.interactive_move_update(&31, (300., 0.).into(), output, to));
        layout.interactive_move_end(&31);
        let dropped = layout.find_workspace_by_id(target_workspace).unwrap().1;
        assert!(dropped.has_window(&31));
        let dropped_layout = dropped
            .tiles_with_ipc_layouts()
            .find(|(tile, _)| tile.window().id() == &31)
            .unwrap()
            .1;
        assert_eq!(
            dropped_layout.pos_in_scrolling_layout.unwrap().0 - 1,
            expected_column
        );
        assert_eq!(shown(&layout, "output1"), source);
        layout.verify_invariants();
    }
}

#[test]
fn empty_hidden_source_is_kept_until_overview_drop_finishes() {
    let mut layout = enabled_layout();
    Op::AddScaledOutput {
        id: 1,
        scale: 1.5,
        layout_config: None,
    }
    .apply(&mut layout);
    Op::AddScaledOutput {
        id: 2,
        scale: 1.0,
        layout_config: None,
    }
    .apply(&mut layout);
    let output = layout
        .outputs()
        .find(|output| output.name() == "output1")
        .unwrap()
        .clone();
    let source = layout.create_collection(None);
    add_window(&mut layout, 41);
    let target = layout.create_collection(Some("destination".into()));
    add_window(&mut layout, 42);
    add_window(&mut layout, 43);
    let current = layout.create_collection(None);
    layout.focus_collection(&WorkspaceReference::Id(current));
    let source_workspace = layout.collection_active_workspace(source).unwrap();
    let target_workspace = layout.collection_active_workspace(target).unwrap();
    layout.open_overview();
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);
    let previews = layout.collection_previews(&output);
    let source_preview = previews
        .iter()
        .find(|preview| preview.id == source)
        .unwrap();
    let target_preview = previews
        .iter()
        .find(|preview| preview.id == target)
        .unwrap();
    let from = source_preview.geometry.loc + source_preview.geometry.size.to_point().downscale(2.);
    let to = target_preview.geometry.loc + target_preview.geometry.size.to_point().downscale(2.);
    let expected = layout
        .find_workspace_by_id(target_workspace)
        .unwrap()
        .1
        .scrolling_insert_position(
            (to - target_preview.geometry.loc).downscale(target_preview.zoom),
        );
    let expected_column = match expected {
        super::super::monitor::InsertPosition::InColumn(column, _)
        | super::super::monitor::InsertPosition::NewColumn(column) => column,
        super::super::monitor::InsertPosition::Floating => panic!("test windows are tiled"),
    };
    assert!(layout.interactive_move_begin(41, &output, from));
    assert!(layout.interactive_move_update(&41, (300., 0.).into(), output, to));
    assert!(layout
        .collections()
        .iter()
        .any(|collection| collection.id == source));
    assert!(layout
        .collection_workspaces(source)
        .all(|workspace| !workspace.has_windows()));
    layout.interactive_move_end(&41);
    assert!(layout
        .find_workspace_by_id(target_workspace)
        .unwrap()
        .1
        .has_window(&41));
    assert!(!layout
        .collections()
        .iter()
        .any(|collection| collection.id == source));
    let moved = layout
        .find_workspace_by_id(target_workspace)
        .unwrap()
        .1
        .tiles_with_ipc_layouts()
        .find(|(tile, _)| tile.window().id() == &41)
        .unwrap()
        .1;
    assert_eq!(
        moved.pos_in_scrolling_layout.unwrap().0 - 1,
        expected_column
    );
    assert!(layout.collection_workspaces(source).count() == 0);
    assert_ne!(source_workspace, target_workspace);
    layout.verify_invariants();
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: ProptestConfig::default().cases.max(128),
        .. ProptestConfig::default()
    })]
    #[test]
    fn actual_drop_target_correctness_property(
        output_index in 0usize..2,
        row_choice in 0usize..2,
        column_choice in 0u8..5,
        x_raw in 4_000u16..=61_535,
        y_raw in 4_000u16..=61_535,
    ) {
        let mut layout = enabled_layout();
        Op::AddScaledOutput { id: 1, scale: 1.0, layout_config: None }.apply(&mut layout);
        Op::AddScaledOutput { id: 2, scale: 1.5, layout_config: None }.apply(&mut layout);
        let outputs: Vec<_> = (1..=2).map(|id| {
            layout.outputs().find(|output| output.name() == format!("output{id}")).unwrap().clone()
        }).collect();
        let mut endpoints = Vec::new();
        for (index, output) in outputs.iter().enumerate() {
            layout.focus_output(output);
            let source = layout.collection_for_output(output).unwrap();
            let moved_window = 100 + index;
            add_window(&mut layout, moved_window);
            let target = layout.create_collection(Some(format!("property-target-{index}")));
            for seed in 0..4 {
                add_window(&mut layout, 200 + index * 10 + seed);
            }
            let target_workspace = layout.collection_active_workspace(target).unwrap();
            layout.monitor_for_output_mut(output).unwrap().add_workspace_bottom();
            layout.move_to_workspace(Some(&(202 + index * 10)), 1, ActivateWindow::No);
            layout.move_to_workspace(Some(&(203 + index * 10)), 1, ActivateWindow::No);
            let second_workspace_id = layout.monitor_for_output(output).unwrap().workspaces[1].id();
            layout.focus_collection(&WorkspaceReference::Id(source));
            let stored_rows: Vec<_> = layout
                .collection_workspaces(target)
                .map(|workspace| workspace.id())
                .collect();
            prop_assert!(stored_rows.contains(&target_workspace));
            prop_assert!(stored_rows.contains(&second_workspace_id));
            for row in [target_workspace, second_workspace_id] {
                let workspace = layout.find_workspace_by_id(row).unwrap().1;
                prop_assert!(workspace.has_windows());
                let columns: Vec<_> = workspace
                    .tiles_with_ipc_layouts()
                    .filter_map(|(_, info)| {
                        info.pos_in_scrolling_layout.map(|(column, _)| column)
                    })
                    .collect();
                prop_assert!(columns.len() >= 2);
                prop_assert!(columns.iter().min() < columns.iter().max());
            }
            endpoints.push((source, target, moved_window, [target_workspace, second_workspace_id]));
        }
        layout.open_overview();
        layout.clock.set_complete_instantly(true);
        layout.advance_animations();

        let (source, target, moved_window, target_rows) = endpoints[output_index];
        let output = outputs[output_index].clone();
        let target_workspace = target_rows[row_choice];
        {
            let collection = layout
                .collections
                .iter_mut()
                .find(|collection| collection.id == target)
                .unwrap();
            collection.active_workspace = Some(target_workspace);
            collection
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id() == target_workspace)
                .unwrap()
                .center_visible_columns();
        }
        let x_fraction = f64::from(x_raw) / 65_535.;
        let y_fraction = f64::from(y_raw) / 65_535.;
        let (start, hovered_screen, expected_column) = {
            let (_, source_geometry, _) = layout
                .collection_window_preview(&output, &moved_window)
                .unwrap();
            let previews = layout.collection_previews(&output);
            let target_preview = previews
                .iter()
                .find(|preview| {
                    preview.id == target && preview.workspace.id() == target_workspace
                })
                .unwrap();
            let target_geometry = target_preview.geometry;
            let target_zoom = target_preview.zoom;
            let target_ws = layout.find_workspace_by_id(target_workspace).unwrap().1;
            let output_view = Rectangle::from_size(super::super::output_size(&output));
            let preview_visible = target_geometry.intersection(output_view).unwrap();
            let preview_local_clip = Rectangle::new(
                (preview_visible.loc - target_geometry.loc).downscale(target_zoom),
                preview_visible.size.downscale(target_zoom),
            );
            let view = Rectangle::from_size(target_ws.view_size())
                .intersection(preview_local_clip)
                .unwrap();
            let x_fraction = (f64::from(column_choice) + x_fraction) / 5.;
            let local = Point::from((
                view.loc.x + view.size.w * x_fraction,
                view.loc.y + view.size.h * y_fraction,
            ));
            let hovered_screen = target_geometry.loc + local.upscale(target_zoom);
            let insert = target_ws.scrolling_insert_position(local);
            let expected_column = match insert {
                super::super::monitor::InsertPosition::InColumn(column, _)
                | super::super::monitor::InsertPosition::NewColumn(column) => column,
                super::super::monitor::InsertPosition::Floating => unreachable!(),
            };
            let start = source_geometry.loc + source_geometry.size.to_point().downscale(2.);
            (start, hovered_screen, expected_column)
        };
        prop_assert!(layout.interactive_move_begin(moved_window, &output, start));
        prop_assert!(layout.interactive_move_update(
            &moved_window,
            (300., 0.).into(),
            output.clone(),
            hovered_screen,
        ));
        prop_assert!(layout.collections.iter().any(|collection| collection.id == source));
        layout.interactive_move_end(&moved_window);
        layout.clock.set_complete_instantly(false);
        let actual_target = layout.collections.iter().find(|collection| {
            collection.id == target
                && collection
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.id() == target_workspace)
        });
        prop_assert!(actual_target.is_some(), "hovered desk/workspace must receive the move");
        let actual_workspace = layout.find_workspace_by_id(target_workspace).unwrap().1;
        prop_assert!(actual_workspace.has_window(&moved_window), "hovered workspace must receive the move");
        let moved_layout = actual_workspace
            .tiles_with_ipc_layouts()
            .find(|(candidate, _)| candidate.window().id() == &moved_window)
            .map(|(_, info)| info)
            .unwrap();
        prop_assert_eq!(moved_layout.pos_in_scrolling_layout.unwrap().0 - 1, expected_column);
        layout.verify_invariants();

    }
}

#[test]
fn keyboard_collection_switch_reuses_swipe_transition_and_is_interruptible() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let first = shown(&layout, "output1");
    let second = layout.create_collection(Some("second".into()));
    let third = layout.create_collection(Some("third".into()));
    layout.focus_collection(&WorkspaceReference::Id(first));

    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    layout.clock.set_unadjusted(Duration::from_millis(100));
    let transition = layout.collection_switch.as_ref().unwrap();
    assert!(transition.offset() > 0.);
    let first_x = transition.offset() + transition.other_offset_delta;
    let second_x = transition.offset();
    assert!(first_x < 0.);

    // The outgoing desk is still visible at its current position when the direction reverses.
    layout.focus_collection_previous();
    assert_eq!(shown(&layout, "output1"), first);
    let transition = layout.collection_switch.as_ref().unwrap();
    assert!((transition.offset() - first_x).abs() < 0.01);
    assert!(transition.offset() < 0.);
    assert!((transition.offset() + transition.other_offset_delta - second_x).abs() < 0.01);

    // Further next presses are resolved from the pending target, so a third desk is not
    // repeatedly replaced by the same second desk while this transition is in flight.
    layout.focus_collection_next();
    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    assert_eq!(
        layout
            .collection_switch_pending
            .as_ref()
            .map(|(_, id, _)| *id),
        Some(third)
    );
    layout.focus_collection_next();
    assert_eq!(
        layout
            .collection_switch_pending
            .as_ref()
            .map(|(_, id, _)| *id),
        Some(first)
    );
    layout.clock.set_unadjusted(Duration::from_secs(5));
    layout.update_collection_switch_render_state();
    assert_eq!(shown(&layout, "output1"), first);
    assert_ne!(shown(&layout, "output1"), third);
}

#[test]
fn two_desk_next_and_previous_use_opposite_wrap_directions() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let output = layout.outputs().next().unwrap().clone();
    let first = shown(&layout, "output1");
    let second = layout.create_collection(Some("second".into()));
    layout.focus_collection(&WorkspaceReference::Id(first));
    let width = layout.monitor_for_output(&output).unwrap().view_size().w;

    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    assert!(layout.collection_switch.as_ref().unwrap().offset() > 0.);
    layout.clock.set_unadjusted(Duration::from_secs(5));
    layout.update_collection_switch_render_state();

    layout.focus_collection_previous();
    assert_eq!(shown(&layout, "output1"), first);
    assert!(layout.collection_switch.as_ref().unwrap().offset() < 0.);
    assert!((layout.collection_switch.as_ref().unwrap().offset().abs() - width).abs() < 0.01);
}

#[test]
fn collection_switch_is_instant_when_animations_are_off() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let first = shown(&layout, "output1");
    let second = layout.create_collection(Some("second".into()));
    layout.focus_collection(&WorkspaceReference::Id(first));
    Rc::make_mut(&mut layout.options).animations.off = true;
    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    assert!(layout.collection_switch.is_none());
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
        layout
            .collections()
            .iter()
            .find(|c| c.id == created)
            .unwrap()
            .name
            .as_deref(),
        Some("research")
    );
    assert!(layout.set_collection_name(&WorkspaceReference::Id(created), Some("work".into())));
    assert!(!layout.set_collection_name(&WorkspaceReference::Index(1), Some("work".into())));
    assert!(layout.set_collection_name(&WorkspaceReference::Id(created), Some("  ".into())));
    assert!(layout.collections().iter().any(|c| c.id == created));
    layout.verify_invariants();
}

#[test]
fn renamed_config_collection_keeps_origin_and_runtime_name_without_duplicates() {
    let mut config = Config::default();
    config.layout.collections.names = vec!["work".into()];
    let mut layout = Layout::new(Clock::with_time(Duration::ZERO), &config);
    Op::AddOutput(1).apply(&mut layout);
    let configured = layout
        .collections()
        .into_iter()
        .find(|collection| collection.name.as_deref() == Some("work"))
        .unwrap()
        .id;

    assert!(layout.set_collection_name(
        &WorkspaceReference::Name("work".into()),
        Some("research".into())
    ));
    layout.update_config(&config);
    assert_eq!(
        layout
            .collections()
            .iter()
            .filter(|collection| collection.name.as_deref() == Some("research"))
            .count(),
        1
    );
    assert_eq!(layout.create_collection(Some("work".into())), configured);

    let another = layout.create_collection(Some("personal".into()));
    assert!(!layout.set_collection_name(&WorkspaceReference::Id(another), Some("work".into())));

    config.layout.collections.names.clear();
    layout.update_config(&config);
    let renamed = layout
        .collections
        .iter()
        .find(|collection| collection.id == configured)
        .unwrap();
    assert_eq!(renamed.name.as_deref(), Some("research"));
    assert!(renamed.persistent);
    assert_eq!(renamed.configured_name, None);
    layout.verify_invariants();
}
