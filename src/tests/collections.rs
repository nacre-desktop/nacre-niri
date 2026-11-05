use niri_config::Config;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;

fn one_window() -> (Fixture, ClientId, WlSurface) {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1280, 720));
    let client = fixture.add_client();
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
    (fixture, client, surface)
}

fn add_window(fixture: &mut Fixture) {
    let client = fixture.add_client();
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
}

#[test]
fn shown_collection_workspaces_have_ipc_monitor_state_after_switch() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1280, 720));
    let ipc_server = crate::ipc::server::IpcServer::start(&fixture.niri().event_loop, None)
        .expect("headless IPC state server");
    let events = ipc_server.subscribe_standard_events();
    fixture.niri().ipc_server = Some(ipc_server);

    let original = fixture.niri().layout.collections()[0].id;
    fixture.niri_state().ipc_refresh_layout();
    while events.try_recv().is_ok() {}
    let shown = fixture.niri().layout.create_collection(None);
    assert_ne!(original, shown);
    fixture.niri_state().ipc_refresh_layout();
    while events.try_recv().is_ok() {}

    // A detached collection is represented in the global workspace query without
    // pretending that it belongs to a monitor.
    let original_workspace_ids = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.id == original)
        .expect("original collection exists")
        .workspace_ids;
    let detached_workspace_id = fixture
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| original_workspace_ids.contains(&ws.id().get()))
        .expect("new collection has a workspace");
    assert!(detached_workspace_id.0.is_none());
    let detached_workspace_id = detached_workspace_id.2.id();
    let ipc_original = fixture
        .niri()
        .ipc_server
        .as_ref()
        .unwrap()
        .workspace_snapshot()
        .into_iter()
        .find(|workspace| workspace.id == detached_workspace_id.get())
        .expect("IPC snapshot has hidden workspace");
    assert!(ipc_original.output.is_none());
    assert!(!ipc_original.is_active);
    assert!(!ipc_original.is_focused);

    // Showing the collection swaps its stack onto the active monitor. These are the
    // values consumed by the IPC WorkspacesChanged snapshot.
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(original));
    fixture.niri_state().ipc_refresh_layout();
    let mut emitted = Vec::new();
    while let Ok(event) = events.try_recv() {
        emitted.push(event);
    }
    let event_workspaces = emitted.into_iter().find_map(|event| match event {
        niri_ipc::Event::WorkspacesChanged { workspaces } => Some(workspaces),
        _ => None,
    });
    let event_workspace = event_workspaces
        .expect("collection switch emits standard WorkspacesChanged")
        .into_iter()
        .find(|workspace| workspace.id == detached_workspace_id.get())
        .expect("event includes shown workspace");
    assert_eq!(event_workspace.output.as_deref(), Some("headless-1"));
    assert!(event_workspace.is_active);
    assert!(event_workspace.is_focused);
    let ipc_shown = fixture
        .niri()
        .ipc_server
        .as_ref()
        .unwrap()
        .workspace_snapshot()
        .into_iter()
        .find(|workspace| workspace.id == detached_workspace_id.get())
        .expect("IPC snapshot retains shown workspace");
    assert_eq!(ipc_shown.output.as_deref(), Some("headless-1"));
    assert!(ipc_shown.is_active);
    assert!(ipc_shown.is_focused);
    let (shown_output, shown_is_active, shown_id) = {
        let shown_workspace = fixture
            .niri()
            .layout
            .workspaces()
            .find(|(_, _, ws)| ws.id() == detached_workspace_id)
            .expect("detached workspace remains addressable after showing");
        (
            shown_workspace
                .0
                .map(|monitor| monitor.output_name().clone()),
            shown_workspace
                .0
                .is_some_and(|monitor| monitor.active_workspace_idx() == shown_workspace.1),
            shown_workspace.2.id(),
        )
    };
    assert_eq!(shown_output.as_deref(), Some("headless-1"));
    assert!(shown_is_active);
    assert_eq!(
        fixture
            .niri()
            .layout
            .active_workspace()
            .map(|workspace| workspace.id()),
        Some(shown_id)
    );

    fixture.niri().layout.open_overview();
    assert!(fixture.niri().layout.is_overview_open());
    fixture.niri().layout.close_overview();
    assert!(!fixture.niri().layout.is_overview_open());
}

#[test]
fn mru_and_overview_survive_hidden_populated_collection() {
    let (mut fixture, _client, _surface) = one_window();
    let original = fixture.niri().layout.collections()[0].id;

    // The original populated collection is hidden and therefore has no output in
    // the workspace query. MRU still constructs safely while another collection
    // is active.
    fixture.niri().layout.create_collection(None);
    fixture.add_output(2, (1024, 768));
    fixture.niri_focus_output(2);
    add_window(&mut fixture);
    let detached_id = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.output.as_deref() == Some("headless-2"))
        .expect("second output collection exists")
        .id;
    let detached_output = fixture.niri_output(2);
    fixture.niri().remove_output(&detached_output);
    let detached_workspace_ids = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.id == detached_id)
        .expect("populated detached collection is retained")
        .workspace_ids;
    assert!(fixture
        .niri()
        .layout
        .workspaces()
        .any(|(monitor, _, ws)| monitor.is_none()
            && detached_workspace_ids.contains(&ws.id().get())
            && ws.has_windows()));
    let hidden_workspace_ids = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.id == original)
        .expect("original collection exists")
        .workspace_ids;
    assert!(
        fixture
            .niri()
            .layout
            .workspaces()
            .any(|(monitor, _, ws)| monitor.is_none()
                && hidden_workspace_ids.contains(&ws.id().get()))
    );

    let niri = fixture.niri();
    let wmru = crate::ui::mru::WindowMru::new(niri);
    assert!(
        wmru.is_empty(),
        "hidden windows are excluded from shown-only MRU"
    );
    niri.layout.open_overview();
    assert!(niri.layout.is_overview_open());
    niri.layout.close_overview();
    assert!(!niri.layout.is_overview_open());
    niri.layout
        .focus_collection(&niri_config::WorkspaceReference::Id(original));
    let wmru = crate::ui::mru::WindowMru::new(niri);
    assert!(!wmru.is_empty());
    let output = niri.layout.active_output().unwrap().clone();
    let clock = niri.clock.clone();
    niri.window_mru_ui.open(clock, wmru, output);
    assert!(niri.window_mru_ui.is_open());
    niri.window_mru_ui
        .advance(niri_config::MruDirection::Forward, None);
    assert!(niri.window_mru_ui.current_window_id().is_some());
    fixture.niri_state().confirm_mru();
    assert!(!fixture.niri().window_mru_ui.is_open());

    let niri = fixture.niri();
    let wmru = crate::ui::mru::WindowMru::new(niri);
    let output = niri.layout.active_output().unwrap().clone();
    niri.window_mru_ui.open(niri.clock.clone(), wmru, output);
    assert!(niri.window_mru_ui.is_open());
    niri.cancel_mru();
    assert!(!niri.window_mru_ui.is_open());
}

#[test]
fn dispatched_workspace_mru_overview_and_screenshot_actions_tolerate_collections() {
    let (mut fixture, _client, _surface) = one_window();
    fixture.niri().layout.create_collection(None);
    fixture.niri().layout.create_collection(None);

    use niri_config::Action;
    let state = fixture.niri_state();
    state.do_action(Action::OpenOverview, false);
    assert!(state.niri.layout.is_overview_open());
    state.do_action(Action::CloseOverview, false);
    assert!(!state.niri.layout.is_overview_open());
    state.do_action(Action::FocusCollectionPrevious, false);
    state.do_action(Action::FocusCollectionNext, false);
    state.do_action(Action::MoveWindowToCollectionPrevious, false);
    state.do_action(Action::MoveWindowToCollectionNext, false);
    state.do_action(Action::ScreenshotScreen(false, false, None), false);
    state.do_action(Action::ScreenshotWindow(false, false, None), false);
    state.do_action(Action::Screenshot(true, None), false);
    assert!(!state.niri.screenshot_ui.is_open());
    state.do_action(Action::CancelScreenshot, false);
    state.do_action(
        Action::MruAdvance {
            direction: niri_config::MruDirection::Forward,
            scope: None,
            filter: None,
        },
        false,
    );
    state.do_action(Action::MruCancel, false);
}

struct ActionContext {
    active_window: u64,
    hidden_window: u64,
    detached_window: u64,
    active_workspace: u64,
    hidden_workspace: u64,
    detached_workspace: u64,
    active_collection: u64,
    hidden_collection: u64,
    detached_collection: u64,
}

fn action_fixture(populated: bool) -> (Fixture, ActionContext) {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    config.layout.collections.names = vec!["home".into(), "kept-empty".into(), "detached".into()];
    let mut fixture = Fixture::with_config(config);
    let ipc = crate::ipc::server::IpcServer::start(&fixture.niri().event_loop, None)
        .expect("headless IPC query state");
    fixture.niri().ipc_server = Some(ipc);
    fixture.niri_state().ipc_keyboard_layouts_changed();
    fixture.add_output(1, (1280, 720));
    if populated {
        add_window(&mut fixture);
    }
    let first_window = if populated {
        fixture.niri().layout.windows().next().unwrap().1.id().get()
    } else {
        0
    };
    fixture.niri().layout.create_collection(None);

    fixture.add_output(2, (1024, 768));
    fixture.niri_focus_output(2);
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Name("detached".into()));
    if populated {
        add_window(&mut fixture);
    }
    let detached_output = fixture.niri_output(2);
    let detached_window = if populated {
        fixture
            .niri()
            .layout
            .windows()
            .find(|(monitor, _)| monitor.is_some_and(|m| m.output_name() == "headless-2"))
            .expect("mapped second-output window")
            .1
            .id()
            .get()
    } else {
        0
    };
    fixture.niri().remove_output(&detached_output);

    fixture.add_output(3, (800, 600));
    // Hotplug intentionally reuses the most recently focused detached desk. Switch to a fresh
    // third-output desk so this action matrix still has a real detached-window target.
    fixture.niri_focus_output(2);
    fixture
        .niri()
        .layout
        .create_collection(Some("dispatch-current".into()));
    fixture.niri_focus_output(1);
    if populated {
        add_window(&mut fixture);
    }
    let active_window = if populated {
        fixture
            .niri()
            .layout
            .windows()
            .find(|(monitor, _)| monitor.is_some_and(|m| m.output_name() == "headless-1"))
            .expect("mapped active window")
            .1
            .id()
            .get()
    } else {
        0
    };

    let workspace_id_for_window = |fixture: &mut Fixture, window_id| {
        fixture
            .niri()
            .layout
            .workspaces()
            .find(|(_, _, ws)| ws.windows().any(|w| w.id().get() == window_id))
            .map(|(_, _, ws)| ws.id().get())
            .expect("window workspace exists")
    };
    let configured_collections = fixture.niri().layout.collections();
    let workspace_in_collection = |name: &str| {
        configured_collections
            .iter()
            .into_iter()
            .find(|collection| collection.name.as_deref() == Some(name))
            .and_then(|collection| collection.workspace_ids.first().copied())
            .expect("configured collection workspace")
    };
    let hidden_workspace = if populated {
        workspace_id_for_window(&mut fixture, first_window)
    } else {
        workspace_in_collection("home")
    };
    let detached_workspace = if populated {
        workspace_id_for_window(&mut fixture, detached_window)
    } else {
        workspace_in_collection("detached")
    };
    fixture.niri().layout.set_workspace_name(
        "home-workspace".into(),
        Some(niri_config::WorkspaceReference::Id(hidden_workspace)),
    );
    let active_workspace = fixture
        .niri()
        .layout
        .active_workspace()
        .expect("active workspace")
        .id()
        .get();

    let workspace_window_state: Vec<_> = fixture
        .niri()
        .layout
        .workspaces()
        .map(|(_, _, ws)| (ws.id().get(), ws.has_windows()))
        .collect();
    let collections = fixture.niri().layout.collections();
    let collection_with_workspace = |workspace_id| {
        collections
            .iter()
            .find(|collection| collection.workspace_ids.contains(&workspace_id))
            .expect("workspace collection exists")
            .id
    };
    let active_collection = collection_with_workspace(active_workspace);
    let hidden_collection = collection_with_workspace(hidden_workspace);
    let detached_collection = collection_with_workspace(detached_workspace);
    assert!(collections.iter().any(|collection| {
        collection.output.is_none() && collection.workspace_ids.contains(&detached_workspace)
    }));
    assert!(fixture.niri().layout.workspaces().any(|(monitor, _, ws)| {
        monitor.is_none() && ws.id().get() == detached_workspace && ws.has_windows() == populated
    }));
    assert!(collections.iter().any(|collection| {
        collection.name.as_deref() == Some("kept-empty")
            && collection.output.is_none()
            && collection.workspace_ids.iter().any(|id| {
                workspace_window_state
                    .iter()
                    .any(|(ws_id, has_windows)| ws_id == id && !has_windows)
            })
    }));

    (
        fixture,
        ActionContext {
            active_window,
            hidden_window: first_window,
            detached_window,
            active_workspace,
            hidden_workspace,
            detached_workspace,
            active_collection,
            hidden_collection,
            detached_collection,
        },
    )
}

#[test]
fn window_workspace_action_dispatch_matrix_covers_variants() {
    use niri_config::{Action, WorkspaceReference as W};
    use niri_ipc::{ColumnDisplay, LayoutSwitchTarget, PositionChange, SizeChange};

    type Case = (&'static str, fn(&ActionContext) -> Action);
    let cases: &[Case] = &[
        ("CloseWindow", |_| Action::CloseWindow),
        ("CloseWindowById", |c| {
            Action::CloseWindowById(c.hidden_window)
        }),
        ("FullscreenWindow", |_| Action::FullscreenWindow),
        ("FullscreenWindowById", |c| {
            Action::FullscreenWindowById(c.detached_window)
        }),
        ("ToggleWindowedFullscreen", |_| {
            Action::ToggleWindowedFullscreen
        }),
        ("ToggleWindowedFullscreenById", |c| {
            Action::ToggleWindowedFullscreenById(c.hidden_window)
        }),
        ("FocusWindow", |c| Action::FocusWindow(c.hidden_window)),
        ("FocusWindowInColumn", |_| Action::FocusWindowInColumn(1)),
        ("FocusWindowPrevious", |_| Action::FocusWindowPrevious),
        ("FocusColumnLeft", |_| Action::FocusColumnLeft),
        ("FocusColumnLeftUnderMouse", |_| {
            Action::FocusColumnLeftUnderMouse
        }),
        ("FocusColumnRight", |_| Action::FocusColumnRight),
        ("FocusColumnRightUnderMouse", |_| {
            Action::FocusColumnRightUnderMouse
        }),
        ("FocusColumnFirst", |_| Action::FocusColumnFirst),
        ("FocusColumnLast", |_| Action::FocusColumnLast),
        ("FocusColumnRightOrFirst", |_| {
            Action::FocusColumnRightOrFirst
        }),
        ("FocusColumnLeftOrLast", |_| Action::FocusColumnLeftOrLast),
        ("FocusColumn", |_| Action::FocusColumn(1)),
        ("FocusWindowOrMonitorUp", |_| Action::FocusWindowOrMonitorUp),
        ("FocusWindowOrMonitorDown", |_| {
            Action::FocusWindowOrMonitorDown
        }),
        ("FocusColumnOrMonitorLeft", |_| {
            Action::FocusColumnOrMonitorLeft
        }),
        ("FocusColumnOrMonitorRight", |_| {
            Action::FocusColumnOrMonitorRight
        }),
        ("FocusWindowDown", |_| Action::FocusWindowDown),
        ("FocusWindowUp", |_| Action::FocusWindowUp),
        ("FocusWindowDownOrColumnLeft", |_| {
            Action::FocusWindowDownOrColumnLeft
        }),
        ("FocusWindowDownOrColumnRight", |_| {
            Action::FocusWindowDownOrColumnRight
        }),
        ("FocusWindowUpOrColumnLeft", |_| {
            Action::FocusWindowUpOrColumnLeft
        }),
        ("FocusWindowUpOrColumnRight", |_| {
            Action::FocusWindowUpOrColumnRight
        }),
        ("FocusWindowOrWorkspaceDown", |_| {
            Action::FocusWindowOrWorkspaceDown
        }),
        ("FocusWindowOrWorkspaceUp", |_| {
            Action::FocusWindowOrWorkspaceUp
        }),
        ("FocusWindowTop", |_| Action::FocusWindowTop),
        ("FocusWindowBottom", |_| Action::FocusWindowBottom),
        ("FocusWindowDownOrTop", |_| Action::FocusWindowDownOrTop),
        ("FocusWindowUpOrBottom", |_| Action::FocusWindowUpOrBottom),
        ("MoveColumnLeft", |_| Action::MoveColumnLeft),
        ("MoveColumnRight", |_| Action::MoveColumnRight),
        ("MoveColumnToFirst", |_| Action::MoveColumnToFirst),
        ("MoveColumnToLast", |_| Action::MoveColumnToLast),
        ("MoveColumnLeftOrToMonitorLeft", |_| {
            Action::MoveColumnLeftOrToMonitorLeft
        }),
        ("MoveColumnRightOrToMonitorRight", |_| {
            Action::MoveColumnRightOrToMonitorRight
        }),
        ("MoveColumnToIndex", |_| Action::MoveColumnToIndex(1)),
        ("MoveWindowDown", |_| Action::MoveWindowDown),
        ("MoveWindowUp", |_| Action::MoveWindowUp),
        ("MoveWindowDownOrToWorkspaceDown", |_| {
            Action::MoveWindowDownOrToWorkspaceDown
        }),
        ("MoveWindowUpOrToWorkspaceUp", |_| {
            Action::MoveWindowUpOrToWorkspaceUp
        }),
        ("ConsumeOrExpelWindowLeft", |_| {
            Action::ConsumeOrExpelWindowLeft
        }),
        ("ConsumeOrExpelWindowLeftById", |c| {
            Action::ConsumeOrExpelWindowLeftById(c.hidden_window)
        }),
        ("ConsumeOrExpelWindowRight", |_| {
            Action::ConsumeOrExpelWindowRight
        }),
        ("ConsumeOrExpelWindowRightById", |c| {
            Action::ConsumeOrExpelWindowRightById(c.detached_window)
        }),
        ("ConsumeWindowIntoColumn", |_| {
            Action::ConsumeWindowIntoColumn
        }),
        ("ExpelWindowFromColumn", |_| Action::ExpelWindowFromColumn),
        ("SwapWindowLeft", |_| Action::SwapWindowLeft),
        ("SwapWindowRight", |_| Action::SwapWindowRight),
        ("ToggleColumnTabbedDisplay", |_| {
            Action::ToggleColumnTabbedDisplay
        }),
        ("SetColumnDisplay", |_| {
            Action::SetColumnDisplay(ColumnDisplay::Tabbed)
        }),
        ("CenterColumn", |_| Action::CenterColumn),
        ("CenterWindow", |_| Action::CenterWindow),
        ("CenterWindowById", |c| {
            Action::CenterWindowById(c.detached_window)
        }),
        ("CenterVisibleColumns", |_| Action::CenterVisibleColumns),
        ("FocusWorkspaceDown", |_| Action::FocusWorkspaceDown),
        ("FocusWorkspaceDownUnderMouse", |_| {
            Action::FocusWorkspaceDownUnderMouse
        }),
        ("FocusWorkspaceUp", |_| Action::FocusWorkspaceUp),
        ("FocusWorkspaceUpUnderMouse", |_| {
            Action::FocusWorkspaceUpUnderMouse
        }),
        ("FocusWorkspaceId", |c| {
            Action::FocusWorkspace(W::Id(c.hidden_workspace))
        }),
        ("FocusWorkspaceIndex", |_| {
            Action::FocusWorkspace(W::Index(1))
        }),
        ("FocusWorkspaceName", |_| {
            Action::FocusWorkspace(W::Name("home-workspace".into()))
        }),
        ("FocusWorkspacePrevious", |_| Action::FocusWorkspacePrevious),
        ("FocusCollectionNext", |_| Action::FocusCollectionNext),
        ("FocusCollectionPrevious", |_| {
            Action::FocusCollectionPrevious
        }),
        ("FocusCollectionId", |c| {
            Action::FocusCollection(W::Id(c.hidden_collection))
        }),
        ("FocusCollectionActive", |c| {
            Action::FocusCollection(W::Id(c.active_collection))
        }),
        ("FocusCollectionActive", |c| {
            Action::FocusCollection(W::Id(c.active_collection))
        }),
        ("FocusCollectionIndex", |_| {
            Action::FocusCollection(W::Index(1))
        }),
        ("FocusCollectionName", |_| {
            Action::FocusCollection(W::Name("home".into()))
        }),
        ("CreateCollection", |_| {
            Action::CreateCollection(Some("matrix".into()))
        }),
        ("MoveWindowToCollectionNext", |_| {
            Action::MoveWindowToCollectionNext
        }),
        ("MoveWindowToCollectionPrevious", |_| {
            Action::MoveWindowToCollectionPrevious
        }),
        ("MoveWindowToCollection", |c| {
            Action::MoveWindowToCollection(W::Id(c.detached_collection))
        }),
        ("MoveWindowToWorkspaceDown", |_| {
            Action::MoveWindowToWorkspaceDown(false)
        }),
        ("MoveWindowToWorkspaceUp", |_| {
            Action::MoveWindowToWorkspaceUp(false)
        }),
        ("MoveWindowToWorkspaceId", |c| {
            Action::MoveWindowToWorkspace(W::Id(c.detached_workspace), false)
        }),
        ("MoveWindowToWorkspaceIndex", |_| {
            Action::MoveWindowToWorkspace(W::Index(1), false)
        }),
        ("MoveWindowToWorkspaceName", |_| {
            Action::MoveWindowToWorkspace(W::Name("home-workspace".into()), false)
        }),
        ("MoveWindowToWorkspaceById", |c| {
            Action::MoveWindowToWorkspaceById {
                window_id: c.hidden_window,
                reference: W::Id(c.active_workspace),
                focus: false,
            }
        }),
        ("MoveColumnToWorkspaceDown", |_| {
            Action::MoveColumnToWorkspaceDown(false)
        }),
        ("MoveColumnToWorkspaceUp", |_| {
            Action::MoveColumnToWorkspaceUp(false)
        }),
        ("MoveColumnToWorkspace", |c| {
            Action::MoveColumnToWorkspace(W::Id(c.active_workspace), false)
        }),
        ("MoveWorkspaceDown", |_| Action::MoveWorkspaceDown),
        ("MoveWorkspaceUp", |_| Action::MoveWorkspaceUp),
        ("MoveWorkspaceToIndex", |_| Action::MoveWorkspaceToIndex(1)),
        ("MoveWorkspaceToIndexByRef", |c| {
            Action::MoveWorkspaceToIndexByRef {
                new_idx: 1,
                reference: W::Id(c.hidden_workspace),
            }
        }),
        ("MoveWorkspaceToMonitorByRef", |c| {
            Action::MoveWorkspaceToMonitorByRef {
                output_name: "headless-3".into(),
                reference: W::Id(c.detached_workspace),
            }
        }),
        ("MoveWorkspaceToMonitor", |_| {
            Action::MoveWorkspaceToMonitor("headless-3".into())
        }),
        ("SetWorkspaceName", |_| {
            Action::SetWorkspaceName("matrix-workspace".into())
        }),
        ("SetWorkspaceNameByRef", |c| Action::SetWorkspaceNameByRef {
            name: "matrix-workspace".into(),
            reference: W::Id(c.hidden_workspace),
        }),
        ("UnsetWorkspaceName", |_| Action::UnsetWorkspaceName),
        ("UnsetWorkSpaceNameByRef", |c| {
            Action::UnsetWorkSpaceNameByRef(W::Id(c.detached_workspace))
        }),
        ("FocusMonitorLeft", |_| Action::FocusMonitorLeft),
        ("FocusMonitorRight", |_| Action::FocusMonitorRight),
        ("FocusMonitorDown", |_| Action::FocusMonitorDown),
        ("FocusMonitorUp", |_| Action::FocusMonitorUp),
        ("FocusMonitorPrevious", |_| Action::FocusMonitorPrevious),
        ("FocusMonitorNext", |_| Action::FocusMonitorNext),
        ("FocusMonitor", |_| {
            Action::FocusMonitor("headless-3".into())
        }),
        ("MoveWindowToMonitorLeft", |_| {
            Action::MoveWindowToMonitorLeft
        }),
        ("MoveWindowToMonitorRight", |_| {
            Action::MoveWindowToMonitorRight
        }),
        ("MoveWindowToMonitorDown", |_| {
            Action::MoveWindowToMonitorDown
        }),
        ("MoveWindowToMonitorUp", |_| Action::MoveWindowToMonitorUp),
        ("MoveWindowToMonitorPrevious", |_| {
            Action::MoveWindowToMonitorPrevious
        }),
        ("MoveWindowToMonitorNext", |_| {
            Action::MoveWindowToMonitorNext
        }),
        ("MoveWindowToMonitor", |_| {
            Action::MoveWindowToMonitor("headless-3".into())
        }),
        ("MoveWindowToMonitorById", |c| {
            Action::MoveWindowToMonitorById {
                id: c.hidden_window,
                output: "headless-3".into(),
            }
        }),
        ("MoveColumnToMonitorLeft", |_| {
            Action::MoveColumnToMonitorLeft
        }),
        ("MoveColumnToMonitorRight", |_| {
            Action::MoveColumnToMonitorRight
        }),
        ("MoveColumnToMonitorDown", |_| {
            Action::MoveColumnToMonitorDown
        }),
        ("MoveColumnToMonitorUp", |_| Action::MoveColumnToMonitorUp),
        ("MoveColumnToMonitorPrevious", |_| {
            Action::MoveColumnToMonitorPrevious
        }),
        ("MoveColumnToMonitorNext", |_| {
            Action::MoveColumnToMonitorNext
        }),
        ("MoveColumnToMonitor", |_| {
            Action::MoveColumnToMonitor("headless-3".into())
        }),
        ("SetWindowWidth", |_| {
            Action::SetWindowWidth(SizeChange::AdjustFixed(10))
        }),
        ("SetWindowWidthById", |c| Action::SetWindowWidthById {
            id: c.active_window,
            change: SizeChange::AdjustFixed(10),
        }),
        ("SetWindowHeight", |_| {
            Action::SetWindowHeight(SizeChange::AdjustFixed(10))
        }),
        ("SetWindowHeightById", |c| Action::SetWindowHeightById {
            id: c.detached_window,
            change: SizeChange::AdjustFixed(10),
        }),
        ("ResetWindowHeight", |_| Action::ResetWindowHeight),
        ("ResetWindowHeightById", |c| {
            Action::ResetWindowHeightById(c.hidden_window)
        }),
        ("SwitchPresetColumnWidth", |_| {
            Action::SwitchPresetColumnWidth
        }),
        ("SwitchPresetColumnWidthBack", |_| {
            Action::SwitchPresetColumnWidthBack
        }),
        ("SwitchPresetWindowWidth", |_| {
            Action::SwitchPresetWindowWidth
        }),
        ("SwitchPresetWindowWidthBack", |_| {
            Action::SwitchPresetWindowWidthBack
        }),
        ("SwitchPresetWindowWidthById", |c| {
            Action::SwitchPresetWindowWidthById(c.detached_window)
        }),
        ("SwitchPresetWindowWidthBackById", |c| {
            Action::SwitchPresetWindowWidthBackById(c.hidden_window)
        }),
        ("SwitchPresetWindowHeight", |_| {
            Action::SwitchPresetWindowHeight
        }),
        ("SwitchPresetWindowHeightBack", |_| {
            Action::SwitchPresetWindowHeightBack
        }),
        ("SwitchPresetWindowHeightById", |c| {
            Action::SwitchPresetWindowHeightById(c.active_window)
        }),
        ("SwitchPresetWindowHeightBackById", |c| {
            Action::SwitchPresetWindowHeightBackById(c.hidden_window)
        }),
        ("MaximizeColumn", |_| Action::MaximizeColumn),
        ("MaximizeWindowToEdges", |_| Action::MaximizeWindowToEdges),
        ("MaximizeWindowToEdgesById", |c| {
            Action::MaximizeWindowToEdgesById(c.detached_window)
        }),
        ("SetColumnWidth", |_| {
            Action::SetColumnWidth(SizeChange::AdjustFixed(10))
        }),
        ("ExpandColumnToAvailableWidth", |_| {
            Action::ExpandColumnToAvailableWidth
        }),
        ("SwitchLayout", |_| {
            Action::SwitchLayout(LayoutSwitchTarget::Next)
        }),
        ("MoveWorkspaceToMonitorLeft", |_| {
            Action::MoveWorkspaceToMonitorLeft
        }),
        ("MoveWorkspaceToMonitorRight", |_| {
            Action::MoveWorkspaceToMonitorRight
        }),
        ("MoveWorkspaceToMonitorDown", |_| {
            Action::MoveWorkspaceToMonitorDown
        }),
        ("MoveWorkspaceToMonitorUp", |_| {
            Action::MoveWorkspaceToMonitorUp
        }),
        ("MoveWorkspaceToMonitorPrevious", |_| {
            Action::MoveWorkspaceToMonitorPrevious
        }),
        ("MoveWorkspaceToMonitorNext", |_| {
            Action::MoveWorkspaceToMonitorNext
        }),
        ("ToggleWindowFloating", |_| Action::ToggleWindowFloating),
        ("ToggleWindowFloatingById", |c| {
            Action::ToggleWindowFloatingById(c.hidden_window)
        }),
        ("MoveWindowToFloating", |_| Action::MoveWindowToFloating),
        ("MoveWindowToFloatingById", |c| {
            Action::MoveWindowToFloatingById(c.detached_window)
        }),
        ("MoveWindowToTiling", |_| Action::MoveWindowToTiling),
        ("MoveWindowToTilingById", |c| {
            Action::MoveWindowToTilingById(c.hidden_window)
        }),
        ("FocusFloating", |_| Action::FocusFloating),
        ("FocusTiling", |_| Action::FocusTiling),
        ("SwitchFocusBetweenFloatingAndTiling", |_| {
            Action::SwitchFocusBetweenFloatingAndTiling
        }),
        ("MoveFloatingWindowById", |c| {
            Action::MoveFloatingWindowById {
                id: Some(c.detached_window),
                x: PositionChange::AdjustFixed(10.),
                y: PositionChange::AdjustFixed(10.),
            }
        }),
        ("ToggleWindowRuleOpacity", |_| {
            Action::ToggleWindowRuleOpacity
        }),
        ("ToggleWindowRuleOpacityById", |c| {
            Action::ToggleWindowRuleOpacityById(c.hidden_window)
        }),
        ("ToggleKeyboardShortcutsInhibit", |_| {
            Action::ToggleKeyboardShortcutsInhibit
        }),
        ("SetDynamicCastWindow", |_| Action::SetDynamicCastWindow),
        ("SetDynamicCastWindowById", |c| {
            Action::SetDynamicCastWindowById(c.detached_window)
        }),
        ("SetDynamicCastMonitor", |_| {
            Action::SetDynamicCastMonitor(Some("headless-3".into()))
        }),
        ("ClearDynamicCastTarget", |_| Action::ClearDynamicCastTarget),
        ("StopCast", |_| Action::StopCast(0)),
        ("ToggleOverview", |_| Action::ToggleOverview),
        ("ShowHotkeyOverlay", |_| Action::ShowHotkeyOverlay),
        ("ToggleWindowUrgent", |c| {
            Action::ToggleWindowUrgent(c.hidden_window)
        }),
        ("SetWindowUrgent", |c| {
            Action::SetWindowUrgent(c.detached_window)
        }),
        ("UnsetWindowUrgent", |c| {
            Action::UnsetWindowUrgent(c.active_window)
        }),
        ("OpenOverview", |_| Action::OpenOverview),
        ("CloseOverview", |_| Action::CloseOverview),
        ("LoadConfigFile", |_| Action::LoadConfigFile(None)),
        ("ScreenshotScreen", |_| {
            Action::ScreenshotScreen(false, false, None)
        }),
        ("ScreenshotWindow", |_| {
            Action::ScreenshotWindow(false, false, None)
        }),
        ("ScreenshotWindowById", |c| Action::ScreenshotWindowById {
            id: c.detached_window,
            write_to_disk: false,
            show_pointer: false,
            path: None,
        }),
        ("Screenshot", |_| Action::Screenshot(true, None)),
        ("ConfirmScreenshot", |_| Action::ConfirmScreenshot {
            write_to_disk: false,
        }),
        ("CancelScreenshot", |_| Action::CancelScreenshot),
        ("ScreenshotTogglePointer", |_| {
            Action::ScreenshotTogglePointer
        }),
        ("MruAdvance", |_| Action::MruAdvance {
            direction: niri_config::MruDirection::Backward,
            scope: Some(niri_config::MruScope::All),
            filter: Some(niri_config::MruFilter::All),
        }),
        ("MruConfirm", |_| Action::MruConfirm),
        ("MruCancel", |_| Action::MruCancel),
        ("MruCloseCurrentWindow", |_| Action::MruCloseCurrentWindow),
        ("MruFirst", |_| Action::MruFirst),
        ("MruLast", |_| Action::MruLast),
        ("MruSetScope", |_| {
            Action::MruSetScope(niri_config::MruScope::Workspace)
        }),
        ("MruCycleScope", |_| Action::MruCycleScope),
    ];

    const EXPECTED_CASES: usize = 188;
    assert_eq!(
        cases.len(),
        EXPECTED_CASES,
        "update the explicit action audit count when adding cases"
    );
    for populated in [true, false] {
        for &(name, make_action) in cases {
            let (mut fixture, context) = action_fixture(populated);
            if populated && name.starts_with("Mru") {
                fixture.niri_state().do_action(
                    Action::MruAdvance {
                        direction: niri_config::MruDirection::Forward,
                        scope: None,
                        filter: None,
                    },
                    false,
                );
            }
            fixture.niri_state().do_action(make_action(&context), false);
            fixture.niri_state().refresh_and_flush_clients();
            let niri = fixture.niri();
            let ipc = niri.ipc_server.as_ref().unwrap();
            assert_eq!(
                ipc.window_snapshot().len(),
                niri.layout.windows().count(),
                "{name}"
            );
            let workspaces = ipc.workspace_snapshot();
            assert_eq!(workspaces.len(), niri.layout.workspaces().count(), "{name}");
            assert!(
                workspaces.iter().all(|workspace| {
                    workspace.output.is_some() || (!workspace.is_active && !workspace.is_focused)
                }),
                "{name}: hidden workspace has active/focused IPC state"
            );
            assert_eq!(
                fixture.niri().layout.windows().count() == 0,
                !populated,
                "{name}"
            );
            assert!(
                fixture.niri().layout.active_output().is_some(),
                "{name} lost active outputs"
            );
            assert!(
                fixture.niri().layout.workspaces().next().is_some(),
                "{name} removed all workspaces"
            );
            if name.starts_with("Screenshot")
                || name == "ConfirmScreenshot"
                || name == "CancelScreenshot"
            {
                assert!(
                    !fixture.niri().screenshot_ui.is_open(),
                    "headless fixture cannot open a renderer-backed screenshot UI: {name}"
                );
            }
        }
    }
}
