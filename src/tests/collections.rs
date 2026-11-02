use niri_config::Config;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;

fn one_window() -> (Fixture, ClientId, WlSurface) {
    let mut fixture = Fixture::new();
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

    let niri = fixture.niri();
    let original = niri.layout.collections()[0].id;
    let shown = niri.layout.create_collection(None);
    assert_ne!(original, shown);

    // A detached collection is represented in the global workspace query without
    // pretending that it belongs to a monitor.
    let detached_workspace_id = niri
        .layout
        .workspaces()
        .find(|(_, _, ws)| {
            niri.layout.collections().iter().any(|collection| {
                collection.id == original && collection.workspace_ids.contains(&ws.id().get())
            })
        })
        .expect("new collection has a workspace");
    assert!(detached_workspace_id.0.is_none());
    let detached_workspace_id = detached_workspace_id.2.id();

    // Showing the collection swaps its stack onto the active monitor. These are the
    // values consumed by the IPC WorkspacesChanged snapshot.
    niri.layout.focus_collection(&niri_config::WorkspaceReference::Id(original));
    let shown_workspace = niri
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
        niri.layout.active_workspace().map(|workspace| workspace.id()),
        Some(shown_workspace.2.id())
    );

    niri.layout.open_overview();
    assert!(niri.layout.is_overview_open());
    niri.layout.close_overview();
    assert!(!niri.layout.is_overview_open());
    niri.layout.verify_invariants();
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
    niri.layout.verify_invariants();
}
