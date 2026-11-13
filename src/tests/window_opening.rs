use std::fmt::{self, Write as _};
use std::time::Instant;

use insta::assert_snapshot;
use niri_config::Config;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use super::*;
use crate::layout::LayoutElement as _;
use crate::utils::spawning::store_and_increase_nofile_rlimit;
use crate::utils::with_toplevel_role;

fn configured_x11_opening_width(
    is_xwayland_satellite: bool,
    proportional_x11_widths: bool,
    min_width: i32,
    max_width: i32,
) -> i32 {
    let mut config = Config::default();
    config.nacre.proportional_x11_widths = proportional_x11_widths;
    config.outputs = niri_config::Outputs(vec![niri_config::Output {
        name: "headless-1".to_owned(),
        scale: Some(niri_config::FloatOrInt(1.5)),
        ..Default::default()
    }]);

    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1920, 1080));

    let id = fixture.add_client_with_xwayland_satellite(is_xwayland_satellite);
    let window = fixture.client(id).create_window();
    let surface = window.surface.clone();
    window.set_min_size(min_width, 300);
    window.set_max_size(max_width, 600);
    window.commit();
    fixture.roundtrip(id);

    fixture
        .client(id)
        .window(&surface)
        .configures_received
        .last()
        .unwrap()
        .1
        .size
        .0
}

#[test]
fn proportional_x11_widths_apply_only_to_managed_adjustable_tiled_clients() {
    let managed = configured_x11_opening_width(true, true, 1000, 1100);
    assert!(
        managed > 0 && managed < 1000,
        "managed X11 width: {managed}"
    );

    let wayland = configured_x11_opening_width(false, true, 1000, 1100);
    assert_eq!(wayland, 1000, "Wayland min width remains effective");

    let disabled = configured_x11_opening_width(true, false, 1000, 1100);
    assert_eq!(disabled, 1000, "config opt-out keeps the X11 min width");

    let fixed = configured_x11_opening_width(true, true, 1000, 1000);
    assert_eq!(fixed, 1000, "fixed-width hints remain effective");
}

#[test]
fn proportional_x11_widths_preserve_max_height_and_floating_constraints() {
    let config = Config {
        outputs: niri_config::Outputs(vec![niri_config::Output {
            name: "headless-1".to_owned(),
            scale: Some(niri_config::FloatOrInt(1.5)),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1920, 1080));

    let id = fixture.add_client_with_xwayland_satellite(true);
    let window = fixture.client(id).create_window();
    let surface = window.surface.clone();
    window.set_min_size(1000, 300);
    window.set_max_size(1100, 600);
    window.commit();
    fixture.roundtrip(id);
    let opening_width = fixture
        .client(id)
        .window(&surface)
        .configures_received
        .last()
        .unwrap()
        .1
        .size
        .0;
    assert!(
        opening_width > 0 && opening_width < 1000,
        "opening width: {opening_width}"
    );

    let window = fixture.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    fixture.double_roundtrip(id);

    let window_id = {
        let niri = fixture.niri();
        let (_, mapped) = niri.layout.windows().next().unwrap();
        assert_eq!(mapped.min_size().w, 1);
        assert_eq!(mapped.max_size().w, 1100);
        assert_eq!(mapped.min_size().h, 300);
        assert_eq!(mapped.max_size().h, 600);
        mapped.window.clone()
    };

    fixture
        .niri()
        .layout
        .set_window_width(Some(&window_id), niri_ipc::SizeChange::SetProportion(100.0));
    fixture.double_roundtrip(id);
    let width = fixture
        .client(id)
        .window(&surface)
        .configures_received
        .last()
        .unwrap()
        .1
        .size
        .0;
    assert!(width <= 1100, "max width should remain in effect: {width}");

    fixture
        .niri()
        .layout
        .set_window_floating(Some(&window_id), true);
    fixture.double_roundtrip(id);
    let floating_width = fixture
        .client(id)
        .window(&surface)
        .configures_received
        .last()
        .unwrap()
        .1
        .size
        .0;
    assert!(
        (1000..=1100).contains(&floating_width),
        "floating width should restore the min/max range: {floating_width}"
    );
    let (min_width, max_width) = {
        let niri = fixture.niri();
        let (_, mapped) = niri.layout.windows().next().unwrap();
        (mapped.min_size().w, mapped.max_size().w)
    };
    assert_eq!(min_width, 1000, "floating restores the X11 minimum");
    assert_eq!(max_width, 1100, "floating keeps the X11 maximum");
}

#[test]
fn simple_no_workspaces() {
    let mut f = Fixture::new();

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 0 × 0, bounds: 0 × 0, states: []"
    );

    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 100 × 688, bounds: 1248 × 688, states: []"
    );
}

#[test]
fn simple() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: []"
    );

    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );
}

#[test]
#[should_panic(expected = "Protocol error 3 on object xdg_surface")]
fn dont_ack_initial_configure() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    // Don't ack the configure.
    window.commit();
    f.double_roundtrip(id);
}

#[derive(Clone, Copy)]
enum WantFullscreen {
    No,
    UnsetBeforeInitial,
    BeforeInitial(Option<&'static str>),
    UnsetAfterInitial,
    AfterInitial(Option<&'static str>),
}

impl fmt::Display for WantFullscreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WantFullscreen::No => write!(f, "U")?,
            WantFullscreen::UnsetBeforeInitial => write!(f, "BU")?,
            WantFullscreen::UnsetAfterInitial => write!(f, "AU")?,
            WantFullscreen::BeforeInitial(m) => write!(f, "B{}", m.unwrap_or("N"))?,
            WantFullscreen::AfterInitial(m) => write!(f, "A{}", m.unwrap_or("N"))?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum WantMaximized {
    No,
    UnsetBeforeInitial,
    BeforeInitial,
    UnsetAfterInitial,
    AfterInitial,
}

impl fmt::Display for WantMaximized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WantMaximized::No => write!(f, "U")?,
            WantMaximized::UnsetBeforeInitial => write!(f, "BU")?,
            WantMaximized::UnsetAfterInitial => write!(f, "AU")?,
            WantMaximized::BeforeInitial => write!(f, "B")?,
            WantMaximized::AfterInitial => write!(f, "A")?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SetParent {
    BeforeInitial(&'static str),
    AfterInitial(&'static str),
}

impl fmt::Display for SetParent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetParent::BeforeInitial(m) => write!(f, "B{m}")?,
            SetParent::AfterInitial(m) => write!(f, "A{m}")?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum DefaultSize {
    WindowChooses,
    Proportion(&'static str),
    Fixed(&'static str),
}

impl fmt::Display for DefaultSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefaultSize::WindowChooses => write!(f, "U"),
            DefaultSize::Proportion(prop) => write!(f, "P{prop}"),
            DefaultSize::Fixed(fixed) => write!(f, "F{fixed}"),
        }
    }
}

#[test]
fn target_output_and_workspaces() {
    store_and_increase_nofile_rlimit();

    // Here we test a massive powerset of settings that can affect where a window opens:
    //
    // * open-on-workspace
    // * open-on-output
    // * has parent (windows will open next to their parent)
    // * want fullscreen (windows can request the target fullscreen output)
    // * open-fullscreen (can deny the fullscreen request)

    let open_on_workspace = [None, Some("1"), Some("2")];
    let open_on_output = [None, Some("1"), Some("2")];
    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::BeforeInitial(Some("1")),
        WantFullscreen::BeforeInitial(Some("2")),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
        WantFullscreen::AfterInitial(Some("1")),
        WantFullscreen::AfterInitial(Some("2")),
    ];
    let set_parent = [
        None,
        Some(SetParent::BeforeInitial("1")),
        Some(SetParent::BeforeInitial("2")),
        Some(SetParent::AfterInitial("1")),
        Some(SetParent::AfterInitial("2")),
    ];

    let mut powerset = Vec::new();
    for ws in open_on_workspace {
        for out in open_on_output {
            for fs in open_fullscreen {
                for wfs in want_fullscreen {
                    for sp in set_parent {
                        powerset.push((ws, out, fs, wfs, sp));
                    }
                }
            }
        }
    }

    powerset.into_par_iter().for_each(|(ws, out, fs, wfs, sp)| {
        check_target_output_and_workspace(ws, out, fs, wfs, sp);
    });
}

#[test]
fn launch_intent_maps_off_view_without_focus_and_rules_still_choose_destination() {
    check_launch_workspace(None, true, false, false);
    check_launch_workspace(Some("ws-rule"), true, false, false);
    check_launch_workspace(Some("ws-rule"), false, false, false);
    check_launch_workspace(None, true, true, false);
    check_launch_workspace(None, true, false, true);
    check_launch_workspace(None, true, true, true);
}

fn check_launch_workspace(
    open_on_workspace: Option<&str>,
    has_launch: bool,
    expire_before_map: bool,
    multi_output: bool,
) {
    store_and_increase_nofile_rlimit();
    let view_output = if multi_output {
        "headless-2"
    } else {
        "headless-1"
    };
    let mut config = Config::parse_mem(&format!(
        r##"
workspace "ws-launch" {{
    open-on-output "headless-1"
}}
workspace "ws-view" {{
    open-on-output "{view_output}"
}}
workspace "ws-rule" {{
    open-on-output "headless-1"
}}
window-rule {{
    match title="launch-test"
    open-focused true
}}
"##,
    ))
    .unwrap();
    if let Some(workspace) = open_on_workspace {
        config = Config::parse_mem(&format!(
            r##"
workspace "ws-launch" {{
    open-on-output "headless-1"
}}
workspace "ws-view" {{
    open-on-output "{view_output}"
}}
workspace "ws-rule" {{
    open-on-output "headless-1"
}}
window-rule {{
    match title="launch-test"
    open-focused true
    open-on-workspace "{workspace}"
}}
"##
        ))
        .unwrap();
    }
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1280, 720));
    if multi_output {
        fixture.add_output(2, (1920, 1080));
    }
    let launch_output = fixture
        .niri()
        .layout
        .monitor_for_workspace("ws-launch")
        .unwrap()
        .output()
        .clone();
    let view_output = fixture
        .niri()
        .layout
        .monitor_for_workspace("ws-view")
        .unwrap()
        .output()
        .clone();
    let (launch_index, launch_workspace) = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-launch")
        .map(|(idx, ws)| (idx, ws.id()))
        .unwrap();
    let (view_index, view_workspace) = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-view")
        .map(|(idx, ws)| (idx, ws.id()))
        .unwrap();
    fixture.niri().layout.focus_output(&view_output);
    fixture.niri().layout.switch_workspace(view_index);
    let anchor_client = fixture.add_client_with_xwayland_satellite(true);
    let anchor_surface = {
        let window = fixture.client(anchor_client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(anchor_client);
    let anchor = fixture.client(anchor_client).window(&anchor_surface);
    anchor.attach_new_buffer();
    anchor.set_size(200, 100);
    anchor.ack_last_and_commit();
    fixture.double_roundtrip(anchor_client);
    let anchor_id = fixture.niri().layout.focus().unwrap().id();
    fixture.niri().layout.focus_output(&launch_output);
    fixture.niri().layout.switch_workspace(launch_index);

    let client_id = fixture.add_client();
    fixture.roundtrip(client_id);
    let surface = fixture.client(client_id).create_window().surface.clone();
    fixture.roundtrip(client_id);
    let server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    let (activation_token, activation_data) = {
        let (token, data) = fixture.niri().activation_state.create_external_token(None);
        (token.clone(), data.clone())
    };
    let token = activation_token.as_str().to_owned();
    let now = std::time::Instant::now();
    if has_launch {
        fixture
            .niri()
            .launch_tracker
            .prepare(token.clone(), None, launch_workspace, now);
        let unmapped = fixture
            .niri()
            .unmapped_windows
            .get_mut(&server_surface)
            .unwrap();
        unmapped.launch_token = Some(token.clone());
        unmapped.launch_workspace_id = Some(launch_workspace);
    }

    if expire_before_map {
        fixture.niri().layout.focus_output(&view_output);
        fixture.niri().layout.switch_workspace(view_index);
    }
    fixture
        .client(client_id)
        .window(&surface)
        .set_title("launch-test");
    fixture.client(client_id).window(&surface).commit();
    fixture.roundtrip(client_id);
    if expire_before_map {
        assert!(fixture
            .niri()
            .launch_tracker
            .peek_token(&token, now + crate::launch::LAUNCH_WORKSPACE_TIMEOUT)
            .is_none());
    } else {
        fixture.niri().layout.focus_output(&view_output);
        fixture.niri().layout.switch_workspace(view_index);
    }
    fixture
        .client(client_id)
        .window(&surface)
        .attach_new_buffer();
    fixture
        .client(client_id)
        .window(&surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(client_id);

    let (mapped_id, mapped_workspace, urgent) = fixture
        .niri()
        .layout
        .workspaces()
        .find_map(|(_, _, workspace)| {
            workspace.windows().find_map(|mapped| {
                with_toplevel_role(mapped.toplevel(), |role| {
                    (role.title.as_deref() == Some("launch-test")).then_some((
                        mapped.id(),
                        workspace.id(),
                        mapped.is_urgent(),
                    ))
                })
            })
        })
        .unwrap();
    let expected_workspace = if expire_before_map {
        view_workspace
    } else {
        open_on_workspace
            .map(|_| {
                fixture
                    .niri()
                    .layout
                    .find_workspace_by_name("ws-rule")
                    .unwrap()
                    .1
                    .id()
            })
            .unwrap_or(launch_workspace)
    };
    assert_eq!(mapped_workspace, expected_workspace);
    if has_launch && !expire_before_map {
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            view_workspace
        );
        assert_eq!(fixture.niri().layout.focus().unwrap().id(), anchor_id);
        assert!(urgent);
    } else {
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            expected_workspace
        );
        assert_eq!(fixture.niri().layout.focus().unwrap().id(), mapped_id);
        assert!(!urgent);
    }

    if has_launch && open_on_workspace.is_some() && !expire_before_map {
        fixture.niri().layout.focus_output(&launch_output);
        fixture.niri().layout.switch_workspace(launch_index);
        <crate::niri::State as smithay::wayland::xdg_activation::XdgActivationHandler>::request_activation(
            fixture.niri_state(),
            activation_token,
            activation_data,
            server_surface,
        );
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            launch_workspace
        );
        assert!(fixture.niri().layout.focus().is_none());
    }
}

#[test]
fn launch_follower_keeps_splash_and_main_toplevels_on_launch_workspace() {
    store_and_increase_nofile_rlimit();
    let mut fixture = Fixture::with_config(
        Config::parse_mem(
            r##"
workspace "ws-launch" {
    open-on-output "headless-1"
}
workspace "ws-view" {
    open-on-output "headless-1"
}
"##,
        )
        .unwrap(),
    );
    fixture.add_output(1, (1280, 720));
    let output = fixture.niri().layout.active_output().unwrap().clone();
    let launch_index = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-launch")
        .unwrap()
        .0;
    let view_index = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-view")
        .unwrap()
        .0;
    let launch_workspace = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-launch")
        .unwrap()
        .1
        .id();
    let view_workspace = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-view")
        .unwrap()
        .1
        .id();
    fixture.niri().layout.focus_output(&output);
    fixture.niri().layout.switch_workspace(view_index);

    let client = fixture.add_client();
    let splash_surface = {
        let splash = fixture.client(client).create_window();
        splash.xdg_toplevel.set_app_id("org.nacre.discord".into());
        splash.set_title("discord-updater-splash");
        splash.surface.clone()
    };
    fixture.roundtrip(client);
    let splash_server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    let token = {
        let (token, _) = fixture.niri().activation_state.create_external_token(None);
        token.as_str().to_owned()
    };
    fixture.niri().launch_tracker.prepare(
        token.clone(),
        Some("nacre-app-discord.service".into()),
        launch_workspace,
        Instant::now(),
    );
    {
        let unmapped = fixture
            .niri()
            .unmapped_windows
            .get_mut(&splash_server_surface)
            .unwrap();
        unmapped.launch_token = Some(token.clone());
        unmapped.launch_workspace_id = Some(launch_workspace);
    }
    fixture.client(client).window(&splash_surface).commit();
    fixture.roundtrip(client);
    fixture
        .client(client)
        .window(&splash_surface)
        .attach_new_buffer();
    fixture
        .client(client)
        .window(&splash_surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(client);

    assert_eq!(
        fixture
            .niri()
            .layout
            .workspaces()
            .find_map(|(_, _, workspace)| {
                workspace.windows().find_map(|window| {
                    with_toplevel_role(window.toplevel(), |role| {
                        (role.title.as_deref() == Some("discord-updater-splash"))
                            .then_some(workspace.id())
                    })
                })
            }),
        Some(launch_workspace)
    );
    assert_eq!(
        fixture.niri().layout.active_workspace().unwrap().id(),
        view_workspace
    );

    fixture.niri().layout.switch_workspace(launch_index);
    fixture.niri().layout.switch_workspace(view_index);
    let main_surface = {
        let main = fixture.client(client).create_window();
        main.xdg_toplevel.set_app_id("org.nacre.discord".into());
        main.set_title("discord-main");
        main.surface.clone()
    };
    fixture.roundtrip(client);
    let main_server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    fixture.client(client).window(&main_surface).commit();
    fixture.roundtrip(client);
    assert_eq!(
        fixture
            .niri()
            .unmapped_windows
            .get(&main_server_surface)
            .unwrap()
            .launch_workspace_id,
        Some(launch_workspace),
        "second main surface should be a launch follower"
    );
    fixture
        .client(client)
        .window(&main_surface)
        .attach_new_buffer();
    fixture
        .client(client)
        .window(&main_surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(client);

    for title in ["discord-updater-splash", "discord-main"] {
        assert_eq!(
            fixture
                .niri()
                .layout
                .workspaces()
                .find_map(|(_, _, workspace)| {
                    workspace.windows().find_map(|window| {
                        with_toplevel_role(window.toplevel(), |role| {
                            (role.title.as_deref() == Some(title)).then_some(workspace.id())
                        })
                    })
                }),
            Some(launch_workspace),
            "{title} must remain on the launch workspace"
        );
    }
    assert_eq!(
        fixture.niri().layout.active_workspace().unwrap().id(),
        view_workspace
    );
    assert!(fixture
        .niri()
        .launch_tracker
        .token_was_consumed(&token, std::time::Instant::now()));
}

#[test]
fn launch_follower_app_id_keeps_same_app_and_leaves_other_apps_at_active_workspace() {
    store_and_increase_nofile_rlimit();
    let mut fixture = Fixture::with_config(
        Config::parse_mem(
            r##"
workspace "ws-launch" {
    open-on-output "headless-1"
}
workspace "ws-view" {
    open-on-output "headless-1"
}
"##,
        )
        .unwrap(),
    );
    fixture.add_output(1, (1280, 720));
    let output = fixture.niri().layout.active_output().unwrap().clone();
    let launch_index = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-launch")
        .unwrap()
        .0;
    let view_index = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-view")
        .unwrap()
        .0;
    let launch_workspace = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-launch")
        .unwrap()
        .1
        .id();
    let view_workspace = fixture
        .niri()
        .layout
        .find_workspace_by_name("ws-view")
        .unwrap()
        .1
        .id();
    fixture.niri().layout.focus_output(&output);
    fixture.niri().layout.switch_workspace(view_index);

    // This simulated satellite has no PID identity; the app-id follower is the only match.
    let app_client = fixture.add_client_with_xwayland_satellite(true);
    let first_surface = {
        let first = fixture.client(app_client).create_window();
        first.xdg_toplevel.set_app_id("org.nacre.discord".into());
        first.set_title("app-first");
        first.surface.clone()
    };
    fixture.roundtrip(app_client);
    let first_server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    let token = {
        let (token, _) = fixture.niri().activation_state.create_external_token(None);
        token.as_str().to_owned()
    };
    fixture.niri().launch_tracker.prepare(
        token.clone(),
        Some("nacre-app-discord.service".into()),
        launch_workspace,
        Instant::now(),
    );
    {
        let unmapped = fixture
            .niri()
            .unmapped_windows
            .get_mut(&first_server_surface)
            .unwrap();
        unmapped.launch_token = Some(token.clone());
        unmapped.launch_workspace_id = Some(launch_workspace);
    }
    fixture.client(app_client).window(&first_surface).commit();
    fixture.roundtrip(app_client);
    fixture
        .client(app_client)
        .window(&first_surface)
        .attach_new_buffer();
    fixture
        .client(app_client)
        .window(&first_surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(app_client);

    fixture.niri().layout.switch_workspace(launch_index);
    fixture.niri().layout.switch_workspace(view_index);
    let second_surface = {
        let second = fixture.client(app_client).create_window();
        second.xdg_toplevel.set_app_id("org.nacre.discord".into());
        second.set_title("app-second");
        second.surface.clone()
    };
    fixture.roundtrip(app_client);
    let second_server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    fixture.client(app_client).window(&second_surface).commit();
    fixture.roundtrip(app_client);
    assert_eq!(
        fixture
            .niri()
            .unmapped_windows
            .get(&second_server_surface)
            .unwrap()
            .launch_workspace_id,
        Some(launch_workspace),
        "same-app surface should match without a PID ancestry bridge"
    );
    fixture
        .client(app_client)
        .window(&second_surface)
        .attach_new_buffer();
    fixture
        .client(app_client)
        .window(&second_surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(app_client);

    let other_client = fixture.add_client();
    let other_surface = {
        let other = fixture.client(other_client).create_window();
        other.xdg_toplevel.set_app_id("org.nacre.other".into());
        other.set_title("other-app");
        other.surface.clone()
    };
    fixture.roundtrip(other_client);
    let other_server_surface = fixture
        .niri()
        .unmapped_windows
        .keys()
        .next()
        .unwrap()
        .clone();
    fixture.client(other_client).window(&other_surface).commit();
    fixture.roundtrip(other_client);
    assert_eq!(
        fixture
            .niri()
            .unmapped_windows
            .get(&other_server_surface)
            .unwrap()
            .launch_workspace_id,
        None
    );
    fixture
        .client(other_client)
        .window(&other_surface)
        .attach_new_buffer();
    fixture
        .client(other_client)
        .window(&other_surface)
        .ack_last_and_commit();
    fixture.double_roundtrip(other_client);

    for (title, expected) in [
        ("app-first", launch_workspace),
        ("app-second", launch_workspace),
        ("other-app", view_workspace),
    ] {
        let actual = fixture
            .niri()
            .layout
            .workspaces()
            .find_map(|(_, _, workspace)| {
                workspace.windows().find_map(|window| {
                    with_toplevel_role(window.toplevel(), |role| {
                        (role.title.as_deref() == Some(title)).then_some(workspace.id())
                    })
                })
            });
        assert_eq!(actual, Some(expected), "{title}");
    }
    assert_eq!(
        fixture.niri().layout.active_workspace().unwrap().id(),
        view_workspace
    );

    let (second_launch_token, activation_data) = {
        let (token, data) = fixture.niri().activation_state.create_external_token(None);
        (token.clone(), data.clone())
    };
    let second_launch_token_value = second_launch_token.as_str().to_owned();
    fixture.niri().launch_tracker.prepare(
        second_launch_token_value.clone(),
        Some("nacre-app-discord-second.service".into()),
        view_workspace,
        Instant::now(),
    );
    <crate::niri::State as smithay::wayland::xdg_activation::XdgActivationHandler>::request_activation(
        fixture.niri_state(),
        second_launch_token,
        activation_data,
        second_server_surface,
    );
    assert!(!fixture
        .niri()
        .launch_tracker
        .token_was_consumed(&token, Instant::now()));
    assert!(fixture
        .niri()
        .launch_tracker
        .peek_token(&second_launch_token_value, Instant::now())
        .is_none());
}

fn check_target_output_and_workspace(
    open_on_workspace: Option<&str>,
    open_on_output: Option<&str>,
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    set_parent: Option<SetParent>,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
output "headless-2" {
    layout {
        border {
            on
        }
    }
}

workspace "ws-1" {
    open-on-output "headless-1"
}

workspace "ws-2" {
    open-on-output "headless-2"

    layout {
        border {
            width 10
        }

        default-column-width {
            fixed 500
        }
    }
}

window-rule {
    exclude title="parent"

"##,
    );

    if let Some(x) = open_on_workspace {
        writeln!(config, "    open-on-workspace \"ws-{x}\"").unwrap();
        snapshot_suffix.push(format!("ws{x}"));
    }

    if let Some(x) = open_on_output {
        writeln!(config, "    open-on-output \"headless-{x}\"").unwrap();
        snapshot_suffix.push(format!("out{x}"));
    }

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }
    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    if let Some(set_parent) = set_parent {
        let mon = match set_parent {
            SetParent::BeforeInitial(mon) => mon,
            SetParent::AfterInitial(mon) => mon,
        };
        write!(
            config,
            "

window-rule {{
    match title=\"parent\"
    open-on-output \"headless-{mon}\"
}}"
        )
        .unwrap();

        snapshot_desc.push(format!("set parent: {set_parent}"));
        snapshot_suffix.push(format!("sp{set_parent}"));
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let mut parent = None;
    if set_parent.is_some() {
        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        parent = Some(window.xdg_toplevel.clone());
        window.set_title("parent");
        window.commit();
        f.roundtrip(id);

        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.ack_last_and_commit();
        f.roundtrip(id);
    }

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let Some(SetParent::BeforeInitial(_)) = set_parent {
        client.window(&surface).set_parent(parent.as_ref());
    }

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let Some(SetParent::AfterInitial(_)) = set_parent {
        client.window(&surface).set_parent(parent.as_ref());
    }

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let niri = f.niri();
    let (mon, ws_idx, ws, mapped) = niri
        .layout
        .workspaces()
        .find_map(|(mon, ws_idx, ws)| {
            ws.windows().find_map(|win| {
                if with_toplevel_role(win.toplevel(), |role| {
                    role.title.as_deref() != Some("parent")
                }) {
                    Some((mon, ws_idx, ws, win))
                } else {
                    None
                }
            })
        })
        .unwrap();
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    let mon = mon.unwrap().output_name().clone();
    let ws = ws.name().cloned().unwrap_or(String::from("unnamed"));

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
final monitor: {mon}
final workspace: {ws_idx} ({ws})

initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}

#[test]
fn target_size() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    store_and_increase_nofile_rlimit();

    // Here we test a massive powerset of settings that can affect the window size:
    //
    // * want fullscreen
    // * open-fullscreen
    // * open-maximized
    // * open-floating
    // * default-column-width
    // * border
    // * default-column-display normal, tabbed

    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
    ];
    let open_maximized = [None, Some("true")];
    let open_floating = [None, Some("true")];
    let default_column_width = [
        None,
        Some(DefaultSize::WindowChooses),
        Some(DefaultSize::Proportion("0.25")),
        Some(DefaultSize::Fixed("1000")),
    ];
    let default_window_height = [
        None,
        Some(DefaultSize::WindowChooses),
        Some(DefaultSize::Proportion("0.5")),
        Some(DefaultSize::Fixed("500")),
    ];
    let border = [false, true];
    let tabbed = [false, true];

    let mut powerset = Vec::new();
    for fs in open_fullscreen {
        for wfs in want_fullscreen {
            for om in open_maximized {
                for of in open_floating {
                    for dw in default_column_width {
                        for dh in default_window_height {
                            for b in border {
                                for t in tabbed {
                                    powerset.push((fs, wfs, om, of, dw, dh, b, t));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    powerset
        .into_par_iter()
        .for_each(|(fs, wfs, om, of, dw, dh, b, t)| {
            check_target_size(fs, wfs, om, of, dw, dh, b, t);
        });
}

#[allow(clippy::too_many_arguments)]
fn check_target_size(
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    open_maximized: Option<&str>,
    open_floating: Option<&str>,
    default_width: Option<DefaultSize>,
    default_height: Option<DefaultSize>,
    border: bool,
    tabbed: bool,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
window-rule {
"##,
    );

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }

    if let Some(x) = open_maximized {
        writeln!(config, "    open-maximized {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("om{x}"));
    }

    if let Some(x) = open_floating {
        writeln!(config, "    open-floating {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("of{x}"));
    }

    if let Some(x) = default_width {
        let value = match x {
            DefaultSize::WindowChooses => String::new(),
            DefaultSize::Proportion(prop) => format!("proportion {prop};"),
            DefaultSize::Fixed(fixed) => format!("fixed {fixed};"),
        };
        writeln!(config, "    default-column-width {{ {value} }}").unwrap();

        snapshot_suffix.push(format!("dw{x}"));
    }

    if let Some(x) = default_height {
        let value = match x {
            DefaultSize::WindowChooses => String::new(),
            DefaultSize::Proportion(prop) => format!("proportion {prop};"),
            DefaultSize::Fixed(fixed) => format!("fixed {fixed};"),
        };
        writeln!(config, "    default-window-height {{ {value} }}").unwrap();

        snapshot_suffix.push(format!("dh{x}"));
    }

    if border {
        writeln!(config, "    border {{ on; }}").unwrap();
        snapshot_suffix.push(String::from("b"));
    }

    if tabbed {
        writeln!(config, "    default-column-display \"tabbed\"").unwrap();
    }

    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    if tabbed {
        config.push_str(
            "\n
layout {
    tab-indicator {
        place-within-column
    }
}",
        );
        snapshot_suffix.push(String::from("t"));
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}

#[test]
fn fullscreen_maximize() {
    store_and_increase_nofile_rlimit();

    let open_fullscreen = [None, Some("false"), Some("true")];
    let want_fullscreen = [
        WantFullscreen::No,
        WantFullscreen::UnsetBeforeInitial, // GTK 4
        WantFullscreen::BeforeInitial(None),
        WantFullscreen::UnsetAfterInitial,
        // mpv, osu!
        WantFullscreen::AfterInitial(None),
    ];
    let open_maximized = [None, Some("false"), Some("true")];
    let want_maximized = [
        WantMaximized::No,
        WantMaximized::UnsetBeforeInitial,
        WantMaximized::BeforeInitial,
        WantMaximized::UnsetAfterInitial,
        WantMaximized::AfterInitial,
    ];

    let mut powerset = Vec::new();
    for fs in open_fullscreen {
        for wfs in want_fullscreen {
            for tm in open_maximized {
                for wm in want_maximized {
                    powerset.push((fs, wfs, tm, wm));
                }
            }
        }
    }

    powerset.into_par_iter().for_each(|(fs, wfs, tm, wm)| {
        check_fullscreen_maximize(fs, wfs, tm, wm);
    });
}

fn check_fullscreen_maximize(
    open_fullscreen: Option<&str>,
    want_fullscreen: WantFullscreen,
    open_maximized: Option<&str>,
    want_maximized: WantMaximized,
) {
    let mut snapshot_desc = Vec::new();
    let mut snapshot_suffix = Vec::new();

    let mut config = String::from(
        r##"
window-rule {
"##,
    );

    if let Some(x) = open_fullscreen {
        writeln!(config, "    open-fullscreen {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("fs{x}"));
    }

    if let Some(x) = open_maximized {
        writeln!(config, "    open-maximized-to-edges {x}").unwrap();

        let x = if x == "true" { "T" } else { "F" };
        snapshot_suffix.push(format!("tm{x}"));
    }

    config.push('}');

    match &want_fullscreen {
        WantFullscreen::No => (),
        x => {
            snapshot_desc.push(format!("want fullscreen: {x}"));
            snapshot_suffix.push(format!("wfs{x}"));
        }
    }

    match &want_maximized {
        WantMaximized::No => (),
        x => {
            snapshot_desc.push(format!("want maximized: {x}"));
            snapshot_suffix.push(format!("wm{x}"));
        }
    }

    snapshot_desc.push(format!("config:{config}"));

    let config = Config::parse_mem(&config).unwrap();

    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    f.add_output(2, (1920, 1080));

    let id = f.add_client();

    // To get output names.
    f.roundtrip(id);

    let client = f.client(id);
    let window = client.create_window();
    let surface = window.surface.clone();

    if let WantMaximized::UnsetBeforeInitial = want_maximized {
        client.window(&surface).unset_maximized();
    } else if let WantMaximized::BeforeInitial = want_maximized {
        client.window(&surface).set_maximized();
    }

    if let WantFullscreen::UnsetBeforeInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::BeforeInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    client.window(&surface).commit();
    f.roundtrip(id);

    let client = f.client(id);
    let initial = client.window(&surface).format_recent_configures();

    if let WantMaximized::UnsetAfterInitial = want_maximized {
        client.window(&surface).unset_maximized();
    } else if let WantMaximized::AfterInitial = want_maximized {
        client.window(&surface).set_maximized();
    }

    if let WantFullscreen::UnsetAfterInitial = want_fullscreen {
        client.window(&surface).unset_fullscreen();
    } else if let WantFullscreen::AfterInitial(mon) = want_fullscreen {
        let output = mon.map(|mon| client.output(&format!("headless-{mon}")));
        client.window(&surface).set_fullscreen(output.as_ref());
    }

    let window = client.window(&surface);
    window.attach_new_buffer();
    let serial = window.configures_received.last().unwrap().0;
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Commit to the post-initial configures.
    let window = f.client(id).window(&surface);
    let new_serial = window.configures_received.last().unwrap().0;
    if new_serial != serial {
        window.ack_last_and_commit();
        f.double_roundtrip(id);
    }

    let window = f.client(id).window(&surface);
    let post_map = window.format_recent_configures();

    // If the window ended up fullscreen, unfullscreen it and output the configure.
    let mut post_unfullscreen = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_fullscreen = mapped.sizing_mode().is_fullscreen();
    let win = mapped.window.clone();
    if is_fullscreen {
        f.niri().layout.set_fullscreen(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unfullscreen = format!(
            "\n\nunfullscreen configure:\n{}",
            window.format_recent_configures()
        );
    }

    // If the window ended up maximized, unmaximize it and output the configure.
    let mut post_unmaximize = String::new();
    let mapped = f.niri().layout.windows().next().unwrap().1;
    let is_maximized = mapped.sizing_mode().is_maximized();
    let win = mapped.window.clone();
    if is_maximized {
        f.niri().layout.set_maximized(&win, false);
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        let window = f.client(id).window(&surface);
        post_unmaximize = format!(
            "\n\nunmaximize configure:\n{}",
            window.format_recent_configures()
        );
    }

    let snapshot = format!(
        "\
initial configure:
{initial}

post-map configures:
{post_map}{post_unfullscreen}{post_unmaximize}",
    );

    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_suffix(snapshot_suffix.join("-"));
    settings.set_description(snapshot_desc.join("\n"));
    let _guard = settings.bind_to_scope();
    assert_snapshot!(snapshot);
}
