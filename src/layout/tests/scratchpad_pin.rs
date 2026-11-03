use super::*;

fn add_window(layout: &mut Layout<TestWindow>, id: usize) {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
    .apply(layout);
}

fn output(layout: &Layout<TestWindow>, id: usize) -> Output {
    layout
        .outputs()
        .find(|output| output.name() == format!("output{id}"))
        .unwrap()
        .clone()
}

#[test]
fn hide_then_toggle_shows_centered_floating_on_focused_output() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.move_window_to_scratchpad(Some(&1));
    assert!(layout.is_window_hidden(&1));
    assert!(!layout
        .windows_for_output(&output(&layout, 1))
        .any(|window| window.id() == &1));

    layout.toggle_scratchpad();
    assert!(!layout.is_window_hidden(&1));
    assert!(layout.scratchpad_active.as_ref() == Some(&1));
    let monitor = layout
        .monitors()
        .find(|monitor| monitor.output_name() == "output2")
        .unwrap();
    assert!(monitor.pinned_workspace.is_floating(&1));
    assert!(monitor.pinned_workspace.has_window(&1));
    let (tile, position, _) = monitor
        .pinned_workspace
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id() == &1)
        .unwrap();
    let area = monitor.pinned_workspace.working_area();
    let expected = area.loc + (area.size.to_point() - tile.tile_size().to_point()).downscale(2.);
    assert!((position.x - expected.x).abs() < 1.);
    assert!((position.y - expected.y).abs() < 1.);
    layout.verify_invariants();
}

#[test]
fn scratchpad_cycles_newest_to_oldest_then_hides() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    for id in 1..=3 {
        add_window(&mut layout, id);
        layout.move_window_to_scratchpad(Some(&id));
    }
    for expected in [3, 2, 1] {
        layout.toggle_scratchpad();
        assert_eq!(layout.scratchpad_active, Some(expected));
    }
    layout.toggle_scratchpad();
    assert!(layout.scratchpad_active.is_none());
    assert!((1..=3).all(|id| layout.is_window_hidden(&id)));
    layout.toggle_scratchpad();
    assert_eq!(layout.scratchpad_active, Some(3));
    layout.verify_invariants();
}

#[test]
fn pinned_window_survives_workspace_collection_and_output_switches() {
    let mut options = Options::default();
    options.layout.collections.enabled = true;
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    assert!(layout.is_window_pinned(&1));
    layout.switch_workspace_down();
    assert!(layout
        .monitors()
        .find(|mon| mon.output_name() == "output1")
        .unwrap()
        .pinned_workspace
        .has_window(&1));

    layout.create_collection(Some("other".into()));
    assert!(layout
        .monitors()
        .find(|mon| mon.output_name() == "output1")
        .unwrap()
        .pinned_workspace
        .has_window(&1));
    let disconnected = output(&layout, 1);
    layout.remove_output(&disconnected);
    assert!(layout.has_window(&1));
    layout.add_output(disconnected, None);
    assert!(layout.is_window_pinned(&1));
    assert!(layout
        .monitors()
        .any(|mon| mon.pinned_workspace.has_window(&1)));
    layout.verify_invariants();
}

#[test]
fn pinned_overlay_moves_to_surviving_output_when_owner_disconnects() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    let unplugged = output(&layout, 2);
    layout.remove_output(&unplugged);
    let remaining = layout.monitors().next().unwrap();
    assert_eq!(remaining.output_name(), "output1");
    assert!(remaining.pinned_workspace.has_window(&1));
    assert!(layout.is_window_pinned(&1));
    layout.verify_invariants();
}

#[test]
fn pinned_window_can_be_unpinned_while_all_outputs_are_disconnected() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    let unplugged = output(&layout, 1);
    layout.remove_output(&unplugged);
    assert!(layout.has_window(&1));
    assert!(layout.is_window_pinned(&1));
    layout.toggle_window_pinned(Some(&1));
    assert!(layout.has_window(&1));
    assert!(!layout.is_window_pinned(&1));
    layout.verify_invariants();
}

#[test]
fn disabling_features_restores_existing_windows_to_normal_layout() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.move_window_to_scratchpad(Some(&1));
    let mut options = Options::default();
    options.nacre.scratchpad = false;
    layout.update_options(options);
    assert!(!layout.is_window_hidden(&1));
    assert!(layout.has_window(&1));

    layout.toggle_window_pinned(Some(&1));
    assert!(layout.is_window_pinned(&1));
    let mut options = Options::default();
    options.nacre.window_pinning = false;
    layout.update_options(options);
    assert!(!layout.is_window_pinned(&1));
    assert!(layout.has_window(&1));
    layout.verify_invariants();
}

#[test]
fn next_to_overlay_windows_never_targets_a_missing_normal_workspace() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    let mut params = TestWindowParams::new(2);
    layout.add_window(
        TestWindow::new(params.clone()),
        AddWindowTarget::NextTo(&1),
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    assert!(layout
        .monitors()
        .next()
        .unwrap()
        .pinned_workspace
        .has_window(&2));

    layout.move_window_to_scratchpad(Some(&1));
    params.id = 3;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::NextTo(&1),
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    assert!(layout.scratchpad.has_window(&3));
    layout.toggle_scratchpad();
    assert_eq!(layout.scratchpad_active, Some(3));
    layout.verify_invariants();
}

#[test]
fn fullscreen_scratchpad_and_open_pinned_windows_remain_visible() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.set_fullscreen(&1, true);
    layout.move_window_to_scratchpad(Some(&1));
    layout.toggle_scratchpad();
    let monitor = layout.monitors().next().unwrap();
    assert!(monitor.pinned_workspace.has_window(&1));
    assert!(monitor
        .pinned_workspace
        .tiles()
        .any(|tile| tile.window().id() == &1));
    layout.set_fullscreen(&1, false);
    layout.refresh(true);
    assert!(layout
        .monitors()
        .next()
        .unwrap()
        .pinned_workspace
        .is_floating(&1));

    let mut params = TestWindowParams::new(2);
    params.rules = Some(ResolvedWindowRules {
        open_pinned: Some(true),
        ..Default::default()
    });
    let window = TestWindow::new(params);
    window.0.pending_sizing_mode.set(SizingMode::Fullscreen);
    layout.add_window(
        window,
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    let monitor = layout.monitors().next().unwrap();
    assert!(monitor.pinned_workspace.has_window(&2));
    assert!(monitor.pinned_workspace.is_floating(&2));
    layout.verify_invariants();
}

#[test]
fn pinned_windows_remain_floating_until_explicitly_unpinned() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    layout.set_fullscreen(&1, true);
    layout.toggle_fullscreen(&1);
    layout.set_maximized(&1, true);
    layout.toggle_maximized(&1);
    layout.set_window_floating(None, false);
    layout.toggle_window_floating(Some(&1));
    let window = layout.focus().unwrap();
    assert_eq!(window.id(), &1);
    assert!(window.pending_sizing_mode().is_normal());
    assert!(layout.is_window_pinned(&1));
    assert!(layout
        .monitors()
        .next()
        .unwrap()
        .pinned_workspace
        .is_floating(&1));
    layout.verify_invariants();
}

#[test]
fn keyboard_focus_can_leave_and_return_to_the_output_overlay() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    add_window(&mut layout, 2);
    layout.focus_floating();
    assert_eq!(layout.focus().unwrap().id(), &1);
    layout.focus_tiling();
    assert_eq!(layout.focus().unwrap().id(), &2);
    layout.switch_focus_floating_tiling();
    assert_eq!(layout.focus().unwrap().id(), &1);
    layout.switch_focus_floating_tiling();
    assert_eq!(layout.focus().unwrap().id(), &2);
    layout.verify_invariants();
}

#[test]
fn unpinning_into_the_last_workspace_keeps_an_empty_tail() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    layout.create_collection(Some("other".into()));
    add_window(&mut layout, 2);
    layout.switch_workspace_down();
    layout.toggle_window_pinned(Some(&1));
    assert!(layout.has_window(&1));
    assert!(!layout.is_window_pinned(&1));
    layout.verify_invariants();
}

#[test]
fn pin_returns_after_last_output_disconnects_from_a_different_collection() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    layout.create_collection(Some("other".into()));
    let disconnected = output(&layout, 1);
    layout.remove_output(&disconnected);
    assert!(layout.has_window(&1));
    Op::AddOutput(2).apply(&mut layout);
    assert!(layout
        .monitors()
        .next()
        .unwrap()
        .pinned_workspace
        .has_window(&1));
    layout.verify_invariants();
}

#[test]
fn dragging_a_pin_to_another_output_keeps_it_sticky() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    let first = output(&layout, 1);
    let second = output(&layout, 2);
    assert!(layout.interactive_move_begin(1, &first, (100., 100.).into()));
    assert!(layout.interactive_move_update(
        &1,
        (100., 0.).into(),
        second.clone(),
        (200., 100.).into()
    ));
    assert!(layout.is_window_pinned(&1));
    layout.interactive_move_end(&1);
    assert!(layout.is_window_pinned(&1));
    assert!(layout
        .monitor_for_output(&second)
        .unwrap()
        .pinned_workspace
        .has_window(&1));
    layout.switch_workspace_down();
    layout.create_collection(Some("after-drag".into()));
    assert!(layout
        .monitor_for_output(&second)
        .unwrap()
        .pinned_workspace
        .has_window(&1));
    layout.verify_invariants();
}

#[test]
fn closing_the_active_overlay_focuses_the_remaining_overlay() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    for id in 1..=2 {
        add_window(&mut layout, id);
        layout.toggle_window_pinned(Some(&id));
    }
    assert_eq!(layout.focus().unwrap().id(), &2);
    layout.remove_window(&2, Transaction::new()).unwrap();
    assert_eq!(layout.focus().unwrap().id(), &1);
    layout.remove_window(&1, Transaction::new()).unwrap();
    assert!(layout.focus().is_none());
    layout.verify_invariants();
}

#[test]
fn explicitly_hiding_a_shown_window_makes_it_most_recent() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    for id in 1..=3 {
        add_window(&mut layout, id);
        layout.move_window_to_scratchpad(Some(&id));
    }
    layout.toggle_scratchpad();
    layout.toggle_scratchpad();
    assert_eq!(layout.scratchpad_active, Some(2));
    layout.move_window_to_scratchpad(None);
    layout.toggle_scratchpad();
    assert_eq!(layout.scratchpad_active, Some(2));
    layout.verify_invariants();
}

#[test]
fn shown_scratchpad_can_return_to_an_ordinary_tiled_workspace() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.move_window_to_scratchpad(None);
    layout.toggle_scratchpad();
    layout.set_window_floating(None, false);
    assert!(layout.scratchpad_active.is_none());
    assert!(!layout.is_window_hidden(&1));
    assert!(!layout.is_window_pinned(&1));
    assert!(layout.active_workspace().unwrap().has_window(&1));
    assert!(!layout.active_workspace().unwrap().is_floating(&1));
    layout.toggle_scratchpad();
    assert!(!layout.is_window_hidden(&1));
    layout.verify_invariants();
}

#[test]
fn pinned_overlay_can_be_resized_interactively() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    assert!(layout.interactive_resize_begin(1, ResizeEdge::BOTTOM_RIGHT));
    assert!(layout.interactive_resize_update(&1, (100., 80.).into()));
    layout.interactive_resize_end(&1);
    assert!(layout.is_window_pinned(&1));
    layout.verify_invariants();
}

#[test]
fn popup_bounds_and_pointer_focus_handle_overlay_and_hidden_windows() {
    let mut layout = Layout::default();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    layout.toggle_window_pinned(Some(&1));
    add_window(&mut layout, 2);
    layout.move_window_to_scratchpad(Some(&2));
    add_window(&mut layout, 3);
    layout.create_collection(Some("hidden-parent".into()));
    let _ = layout.popup_target_rect(&1);
    let _ = layout.popup_target_rect(&2);
    let _ = layout.popup_target_rect(&3);
    assert!(layout.should_trigger_focus_follows_mouse_on(&1));
    assert!(!layout.should_trigger_focus_follows_mouse_on(&2));
    assert!(!layout.should_trigger_focus_follows_mouse_on(&3));
    layout.verify_invariants();
}
