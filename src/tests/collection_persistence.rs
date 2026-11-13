use niri_config::Config;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;
use crate::layout::collection_persistence::{
    PersistedCollection, PersistedCollections, PersistedWindowRoute, PersistedWorkspace,
    PersistedWorkspaceRef,
};
use crate::layout::workspace::WorkspaceId;
use crate::utils::with_toplevel_role;

fn mapped_window(fixture: &mut Fixture, app_id: &str, title: &str) -> (ClientId, WlSurface) {
    let client = fixture.add_client();
    let surface = {
        let window = fixture.client(client).create_window();
        window.xdg_toplevel.set_app_id(app_id.to_owned());
        window.set_title(title);
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
    (client, surface)
}

fn assignment(
    fixture: &Fixture,
    app_id: &str,
    title: &str,
) -> (Option<String>, Option<WorkspaceId>) {
    let mut result = None;
    fixture
        .state
        .server
        .state
        .niri
        .layout
        .with_windows(|window, output, workspace_id, _| {
            if with_toplevel_role(window.toplevel(), |role| {
                role.app_id.as_deref() == Some(app_id) && role.title.as_deref() == Some(title)
            }) {
                result = Some((output.map(|output| output.name()), workspace_id));
            }
        });
    result.expect("window is mapped")
}

#[test]
fn remembered_hidden_workspace_routes_apply_unless_config_has_explicit_placement() {
    let config = Config::parse_mem(
        r#"
layout {
    collections {
        on
        collection "work" {}
        collection "explicit" {}
    }
}
workspace "configured" {}
window-rule {
    match app-id="org.example.Explicit"
    open-on-workspace "configured"
}
window-rule {
    match app-id="org.example.Output"
    open-on-output "headless-2"
}
"#,
    )
    .unwrap();
    let mut fixture = Fixture::with_config(config);

    let mut state = PersistedCollections::empty();
    state.collections.push(PersistedCollection {
        name: "hidden".into(),
        configured_origin: None,
        original_output: None,
        workspaces: vec![
            PersistedWorkspace {
                name: None,
                retain_empty: true,
            },
            PersistedWorkspace {
                name: None,
                retain_empty: true,
            },
        ],
        active_workspace: 0,
    });
    state.window_routes = vec![
        PersistedWindowRoute {
            app_id: Some("org.example.Remembered".into()),
            title: Some("Remembered".into()),
            collection: "hidden".into(),
            workspace: PersistedWorkspaceRef::Ordinal(1),
        },
        PersistedWindowRoute {
            app_id: Some("org.example.Explicit".into()),
            title: Some("Explicit".into()),
            collection: "hidden".into(),
            workspace: PersistedWorkspaceRef::Ordinal(0),
        },
        PersistedWindowRoute {
            app_id: Some("org.example.Output".into()),
            title: Some("Output".into()),
            collection: "hidden".into(),
            workspace: PersistedWorkspaceRef::Ordinal(0),
        },
    ];
    {
        let niri = fixture.niri();
        assert!(niri.layout.restore_persisted_collections(&mut state));
        niri.collection_persistence = Some(state.clone());
    }
    fixture.add_output(1, (1280, 720));
    fixture.add_output(2, (1280, 720));

    let remembered_route = state
        .route_for_window(Some("org.example.Remembered"), Some("Remembered"))
        .unwrap();
    let remembered_workspace = fixture
        .niri()
        .layout
        .persisted_workspace_id(
            &state,
            &remembered_route.collection,
            &remembered_route.workspace,
        )
        .unwrap();
    mapped_window(&mut fixture, "org.example.Remembered", "Remembered");
    let (remembered_output, remembered_target) =
        assignment(&fixture, "org.example.Remembered", "Remembered");
    assert_eq!(remembered_output, None);
    assert_eq!(remembered_target, Some(remembered_workspace));

    mapped_window(&mut fixture, "org.example.Explicit", "Explicit");
    let (_, explicit_target) = assignment(&fixture, "org.example.Explicit", "Explicit");
    let configured_workspace = fixture
        .niri()
        .layout
        .find_workspace_by_name("configured")
        .map(|(_, workspace)| workspace.id())
        .expect("configured workspace exists");
    assert_eq!(explicit_target, Some(configured_workspace));

    mapped_window(&mut fixture, "org.example.Output", "Output");
    let (explicit_output, explicit_output_workspace) =
        assignment(&fixture, "org.example.Output", "Output");
    assert_eq!(explicit_output.as_deref(), Some("headless-2"));
    assert_ne!(explicit_output_workspace, Some(remembered_workspace));

    fixture.niri().persist_collection_state();
    let persisted = fixture
        .niri()
        .collection_persistence
        .as_ref()
        .expect("in-memory persistence remains active in the fixture");
    assert!(persisted
        .route_for_window(Some("org.example.Remembered"), Some("Remembered"))
        .is_some());
    assert!(persisted
        .route_for_window(Some("org.example.Explicit"), Some("Explicit"))
        .is_some());
    assert!(persisted
        .route_for_window(Some("org.example.Output"), Some("Output"))
        .is_some());
}

#[test]
fn persistence_timer_arms_only_on_change() {
    // Collections are on by default; a running open animation would move the workspace
    // revision on every refresh.
    let mut config = Config::default();
    config.animations.off = true;
    let mut fixture = Fixture::with_config(config);
    let mut state = PersistedCollections::empty();
    state.collections.push(PersistedCollection {
        name: "kept".into(),
        configured_origin: None,
        original_output: None,
        workspaces: vec![PersistedWorkspace {
            name: None,
            retain_empty: true,
        }],
        active_workspace: 0,
    });
    {
        let niri = fixture.niri();
        assert!(niri.layout.restore_persisted_collections(&mut state));
        niri.collection_persistence = Some(state);
    }
    fixture.add_output(1, (1280, 720));
    let (client, surface) = mapped_window(&mut fixture, "org.example.Kept", "Kept");

    // Returns whether the one-shot save timer was armed, and disarms it.
    let disarm = |fixture: &mut Fixture| {
        let niri = fixture.niri();
        let token = niri.collection_persist_timer.take();
        if let Some(token) = token {
            niri.event_loop.remove(token);
        }
        token.is_some()
    };
    disarm(&mut fixture);
    fixture.double_roundtrip(client);
    assert!(
        !disarm(&mut fixture),
        "idle refreshes must not arm the save timer"
    );

    // Titles can change without a commit, so no workspace revision moves.
    fixture.client(client).window(&surface).set_title("Renamed");
    fixture.double_roundtrip(client);
    assert!(disarm(&mut fixture), "a title change arms the save timer");
}
