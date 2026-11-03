use super::super::collection_persistence::{
    PersistedCollection, PersistedCollections, PersistedWindowRoute, PersistedWorkspace,
    PersistedWorkspaceRef,
};
use super::*;

#[test]
fn restored_unnamed_slots_survive_output_cleanup_and_keep_window_routes() {
    let mut state = PersistedCollections::empty();
    state.collections.push(PersistedCollection {
        name: "work".into(),
        configured_origin: None,
        original_output: Some("output1".into()),
        workspaces: vec![
            PersistedWorkspace {
                name: None,
                retain_empty: true,
            },
            PersistedWorkspace {
                name: None,
                retain_empty: true,
            },
            PersistedWorkspace {
                name: None,
                retain_empty: true,
            },
        ],
        active_workspace: 2,
    });
    state.window_routes = vec![
        PersistedWindowRoute {
            app_id: Some("org.example.First".into()),
            title: Some("First".into()),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Ordinal(0),
        },
        PersistedWindowRoute {
            app_id: Some("org.example.Last".into()),
            title: Some("Last".into()),
            collection: "work".into(),
            workspace: PersistedWorkspaceRef::Ordinal(2),
        },
    ];
    for empty_above_first in [false, true] {
        let mut options = Options::default();
        options.layout.collections.enabled = true;
        options.layout.empty_workspace_above_first = empty_above_first;
        let mut state = state.clone();
        for _ in 0..3 {
            let mut layout =
                Layout::with_options(Clock::with_time(Duration::ZERO), options.clone());
            assert!(layout.restore_persisted_collections(&mut state));

            add_output(&mut layout);
            layout.verify_invariants();

            let snapshot = layout.persisted_collection_snapshot();
            assert_eq!(snapshot.len(), 1);
            assert_eq!(snapshot[0].workspaces.len(), 3);
            assert_eq!(snapshot[0].active_workspace, 2);

            let first_route = state
                .route_for_window(Some("org.example.First"), Some("First"))
                .unwrap();
            let first_workspace = layout
                .persisted_workspace_id(&state, &first_route.collection, &first_route.workspace)
                .unwrap();
            let last_route = state
                .route_for_window(Some("org.example.Last"), Some("Last"))
                .unwrap();
            let last_workspace = layout
                .persisted_workspace_id(&state, &last_route.collection, &last_route.workspace)
                .unwrap();

            let _ = layout.add_window(
                TestWindow::new(TestWindowParams::new(1)),
                AddWindowTarget::Workspace(first_workspace),
                None,
                None,
                false,
                false,
                ActivateWindow::No,
            );
            let _ = layout.add_window(
                TestWindow::new(TestWindowParams::new(3)),
                AddWindowTarget::Workspace(last_workspace),
                None,
                None,
                false,
                false,
                ActivateWindow::No,
            );
            let mut assignments = std::collections::HashMap::new();
            layout.with_windows(|window, _, workspace_id, _| {
                assignments.insert(*window.id(), workspace_id);
            });
            assert_eq!(assignments.get(&1), Some(&Some(first_workspace)));
            assert_eq!(assignments.get(&3), Some(&Some(last_workspace)));

            state.collections = snapshot;
            layout.refresh_persisted_workspace_ids(&mut state);
            assert_eq!(
                layout.persisted_workspace_id(&state, "work", &PersistedWorkspaceRef::Ordinal(2)),
                Some(last_workspace)
            );
            layout.verify_invariants();
        }
    }
}

#[test]
fn empty_named_collection_keeps_one_workspace_without_growing_placeholders() {
    let mut base_state = PersistedCollections::empty();
    base_state.collections.push(PersistedCollection {
        name: "work".into(),
        configured_origin: None,
        original_output: None,
        workspaces: vec![PersistedWorkspace {
            name: None,
            retain_empty: false,
        }],
        active_workspace: 0,
    });

    for empty_above_first in [false, true] {
        let mut options = Options::default();
        options.layout.collections.enabled = true;
        options.layout.empty_workspace_above_first = empty_above_first;
        let mut state = base_state.clone();
        for _ in 0..3 {
            let mut layout =
                Layout::with_options(Clock::with_time(Duration::ZERO), options.clone());
            assert!(layout.restore_persisted_collections(&mut state));
            add_output(&mut layout);
            let collection_id = layout.collections[0].id;
            assert_eq!(layout.collection_workspaces(collection_id).count(), 1);
            let snapshot = layout.persisted_collection_snapshot();
            assert_eq!(snapshot[0].workspaces.len(), 1);
            assert_eq!(snapshot[0].active_workspace, 0);
            state.collections = snapshot;
            layout.refresh_persisted_workspace_ids(&mut state);
        }
    }
}

#[test]
fn malformed_duplicate_workspace_state_is_rejected_without_mutating_layout() {
    let mut options = Options::default();
    options.layout.collections.enabled = true;
    let mut layout = Layout::<TestWindow>::with_options(Clock::with_time(Duration::ZERO), options);
    layout.create_collection(Some("kept".into()));
    let before = layout.persisted_collection_snapshot();

    let mut malformed = PersistedCollections::empty();
    malformed.collections = vec![
        PersistedCollection {
            name: "one".into(),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: Some("same".into()),
                retain_empty: false,
            }],
            active_workspace: 0,
        },
        PersistedCollection {
            name: "two".into(),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: Some("SAME".into()),
                retain_empty: false,
            }],
            active_workspace: 0,
        },
    ];
    assert!(!layout.restore_persisted_collections(&mut malformed));
    assert_eq!(layout.persisted_collection_snapshot(), before);
}

#[test]
fn restored_config_workspace_is_removed_from_pending_output_placement() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    config.layout.collections.names = vec!["primary".into(), "hidden".into()];
    config.workspaces.push(niri_config::Workspace {
        name: niri_config::workspace::WorkspaceName("CODE".into()),
        open_on_output: Some("output1".into()),
        layout: None,
    });
    let mut layout = Layout::new(Clock::with_time(Duration::ZERO), &config);

    let mut state = PersistedCollections::empty();
    state.collections.push(PersistedCollection {
        name: "hidden".into(),
        configured_origin: Some("hidden".into()),
        original_output: None,
        workspaces: vec![PersistedWorkspace {
            name: Some("code".into()),
            retain_empty: true,
        }],
        active_workspace: 0,
    });
    assert!(layout.restore_persisted_collections(&mut state));
    let hidden_id = layout
        .collections()
        .iter()
        .find(|collection| collection.name.as_deref() == Some("hidden"))
        .unwrap()
        .id;
    let code_id = layout
        .collection_workspaces(hidden_id)
        .find(|workspace| workspace.name().is_some_and(|name| name == "CODE"))
        .unwrap()
        .id();
    assert!(!layout.collection_pending_workspaces.contains(&code_id));

    add_output(&mut layout);
    assert!(layout
        .collection_workspaces(hidden_id)
        .any(|workspace| workspace.id() == code_id));
    layout.verify_invariants();
}

#[test]
fn first_session_closed_window_route_keeps_its_unnamed_slot_for_restart() {
    let mut options = Options::default();
    options.layout.collections.enabled = true;
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options.clone());
    layout.create_collection(Some("work".into()));
    add_output(&mut layout);
    let _ = layout.add_window(
        TestWindow::new(TestWindowParams::new(7)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::No,
    );
    let first_workspace = layout.active_workspace().unwrap().id();
    let (collection, workspace) = layout
        .persisted_workspace_location(first_workspace)
        .expect("window is in persistent named collection");

    let mut state = PersistedCollections::empty();
    state.collections = layout.persisted_collection_snapshot();
    state.window_routes.push(PersistedWindowRoute {
        app_id: Some("org.example.Closed".into()),
        title: Some("Closed".into()),
        collection,
        workspace,
    });
    layout.refresh_persisted_workspace_ids(&mut state);
    state.retain_routed_empty_workspaces();
    layout.retain_persisted_workspace_slots(&state);

    layout.remove_window(&7, Transaction::new());
    state.collections = layout.persisted_collection_snapshot();
    layout.refresh_persisted_workspace_ids(&mut state);
    assert_eq!(state.window_routes.len(), 1);
    assert_eq!(state.collections[0].workspaces.len(), 1);
    assert!(state.collections[0].workspaces[0].retain_empty);

    let mut restored = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    assert!(restored.restore_persisted_collections(&mut state));
    add_output(&mut restored);
    let route = state
        .route_for_window(Some("org.example.Closed"), Some("Closed"))
        .unwrap();
    let target = restored
        .persisted_workspace_id(&state, &route.collection, &route.workspace)
        .expect("closed app target survives initial restart");
    assert!(restored
        .collection_workspaces(
            restored
                .collections()
                .iter()
                .find(|collection| collection.name.as_deref() == Some("work"))
                .unwrap()
                .id,
        )
        .any(|workspace| workspace.id() == target));
    restored.verify_invariants();
}

fn add_output(layout: &mut Layout<TestWindow>) {
    let output = Output::new(
        "output1".into(),
        PhysicalProperties {
            size: Size::from((1280, 720)),
            subpixel: Subpixel::Unknown,
            make: String::new(),
            model: String::new(),
            serial_number: String::new(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: Size::from((1280, 720)),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    output.user_data().insert_if_missing(|| OutputName {
        connector: "output1".into(),
        make: None,
        model: None,
        serial: None,
    });
    layout.add_output(output, None);
}
