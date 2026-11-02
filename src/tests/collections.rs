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

#[test]
fn shown_collection_workspaces_have_ipc_monitor_state_after_switch() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1280, 720));
    let ipc_server = crate::ipc::server::IpcServer::start(&fixture.handle, None)
        .expect("headless IPC state server");
    fixture.niri().ipc_server = Some(ipc_server);

    let original = fixture.niri().layout.collections()[0].id;
    fixture.niri_state().ipc_refresh_layout();
    let shown = fixture.niri().layout.create_collection(None);
    assert_ne!(original, shown);
    fixture.niri_state().ipc_refresh_layout();

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
    let shown_workspace = fixture
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| ws.id() == detached_workspace_id)
        .expect("detached workspace remains addressable after showing");
    assert_eq!(
        shown_workspace
            .0
            .map(|monitor| monitor.output_name().as_str()),
        Some("headless-1")
    );
    assert!(shown_workspace.0.is_some_and(|monitor| {
        monitor.active_workspace_idx() == shown_workspace.1
    }));
    assert_eq!(
        fixture
            .niri()
            .layout
            .active_workspace()
            .map(|workspace| workspace.id()),
        Some(shown_workspace.2.id())
    );

    fixture.niri().layout.open_overview();
    assert!(fixture.niri().layout.is_overview_open());
    fixture.niri().layout.close_overview();
    assert!(!fixture.niri().layout.is_overview_open());
    fixture.niri().layout.verify_invariants();
}

#[test]
fn mru_and_overview_survive_hidden_populated_and_detached_empty_collections() {
    let (mut fixture, _client, _surface) = one_window();
    let original = fixture.niri().layout.collections()[0].id;
    fixture.niri().layout.create_collection(None);

    // The original populated workspace is hidden. A further collection is detached and empty.
    let detached = fixture.niri().layout.create_collection(None);
    let detached_workspace_ids = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.id == detached)
        .expect("detached collection exists")
        .workspace_ids;
    let empty_detached_workspace = fixture
        .niri()
        .layout
        .workspaces()
        .find(|(_, _, ws)| detached_workspace_ids.contains(&ws.id().get()))
        .map(|(_, _, ws)| ws.id())
        .expect("detached collection has an empty workspace");
    assert!(fixture
        .niri()
        .layout
        .find_workspace_by_id(empty_detached_workspace)
        .is_some());

    let niri = fixture.niri();
    let wmru = crate::ui::mru::WindowMru::new(niri);
    assert!(wmru.is_empty(), "hidden windows are excluded from shown-only MRU");
    niri.layout.open_overview();
    assert!(niri.layout.is_overview_open());
    niri.layout.close_overview();
    assert!(!niri.layout.is_overview_open());
    niri.layout.focus_collection(&niri_config::WorkspaceReference::Id(original));
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
    niri.layout.verify_invariants();
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
    state.do_action(
        Action::MruAdvance {
            direction: niri_config::MruDirection::Forward,
            scope: None,
            filter: None,
        },
        false,
    );
    state.do_action(Action::MruCancel, false);
    state.niri.layout.verify_invariants();
}
