//! End-to-end action, lifecycle and IPC coverage for native hidden/sticky windows.

use niri_config::{Action, Config};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;

fn window(config: Config) -> (Fixture, ClientId, WlSurface) {
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    let client = f.add_client();
    let surface = {
        let window = f.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    f.roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(200, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let server = crate::ipc::server::IpcServer::start(&f.niri().event_loop, None).unwrap();
    f.niri().ipc_server = Some(server);
    f.niri_state().ipc_keyboard_layouts_changed();
    f.niri_state().ipc_refresh_layout();
    (f, client, surface)
}

fn snapshot(f: &mut Fixture) -> niri_ipc::Window {
    f.niri_state().ipc_refresh_layout();
    let windows = f.niri().ipc_server.as_ref().unwrap().window_snapshot();
    assert_eq!(windows.len(), 1);
    windows.into_iter().next().unwrap()
}

#[test]
fn hide_show_ipc_mru_and_unmap() {
    let (mut f, client, surface) = window(Config::default());
    let id = snapshot(&mut f).id;
    let events = f
        .niri()
        .ipc_server
        .as_ref()
        .unwrap()
        .subscribe_standard_events();
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    f.double_roundtrip(client);
    assert!(snapshot(&mut f).is_hidden);
    assert!(
        std::iter::from_fn(|| events.try_recv().ok()).any(|event| matches!(event,
            niri_ipc::Event::WindowOpenedOrChanged { window } if window.id == id && window.is_hidden
        ))
    );
    assert!(crate::ui::mru::WindowMru::new(f.niri()).is_empty());
    f.add_output(2, (1024, 768));
    f.niri_focus_output(2);
    f.niri_state().do_action(Action::ToggleScratchpad, false);
    f.double_roundtrip(client);
    let shown = snapshot(&mut f);
    assert!(!shown.is_hidden);
    assert!(shown.is_floating);
    assert_eq!(shown.id, id);
    let output = f.niri_output(2);
    assert!(f
        .niri()
        .layout
        .windows_for_output(&output)
        .any(|w| w.id().get() == id));
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpadById(id), false);
    assert!(snapshot(&mut f).is_hidden);
    f.niri_state()
        .do_action(Action::ScratchpadShowWindow(id), false);
    assert!(!snapshot(&mut f).is_hidden);
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    f.client(client).window(&surface).attach_null();
    f.client(client).window(&surface).commit();
    f.double_roundtrip(client);
    f.niri_state().ipc_refresh_layout();
    assert!(f
        .niri()
        .ipc_server
        .as_ref()
        .unwrap()
        .window_snapshot()
        .is_empty());
    f.niri_state().do_action(Action::ToggleScratchpad, false);
}

#[test]
fn pin_action_ipc_survives_workspace_and_collection_switches() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let (mut f, client, _) = window(config);
    let output = f.niri_output(1);
    f.niri_state().do_action(Action::ToggleWindowPinned, false);
    f.double_roundtrip(client);
    let pinned = snapshot(&mut f);
    assert!(pinned.is_pinned);
    assert!(pinned.is_floating);
    assert!(!crate::ui::mru::WindowMru::new(f.niri()).is_empty());
    f.niri_state().do_action(Action::FocusWorkspaceDown, false);
    f.niri_complete_animations();
    assert!(snapshot(&mut f).is_pinned);
    assert!(f
        .niri()
        .layout
        .windows_for_output(&output)
        .any(|w| w.id().get() == pinned.id));
    f.niri_state()
        .do_action(Action::CreateCollection(Some("other".into())), false);
    f.niri_complete_animations();
    assert!(snapshot(&mut f).is_pinned);
    assert!(f
        .niri()
        .layout
        .windows_for_output(&output)
        .any(|w| w.id().get() == pinned.id));
    f.niri_state()
        .do_action(Action::ToggleWindowPinnedById(pinned.id), false);
    assert!(!snapshot(&mut f).is_pinned);
}

#[test]
fn native_flags_disable_actions_and_open_pinned_rule() {
    let mut config = Config::parse_mem("window-rule { open-pinned true; }").unwrap();
    config.nacre.scratchpad = false;
    config.nacre.window_pinning = false;
    let (mut f, _, _) = window(config);
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    f.niri_state().do_action(Action::ToggleWindowPinned, false);
    let win = snapshot(&mut f);
    assert!(!win.is_hidden);
    assert!(!win.is_pinned);
    assert!(!win.is_floating);
}

#[test]
fn open_pinned_rule_applies_on_map() {
    let config = Config::parse_mem("window-rule { open-pinned true; }").unwrap();
    let (mut f, _, _) = window(config);
    let win = snapshot(&mut f);
    assert!(win.is_pinned);
    assert!(win.is_floating);
}

#[test]
fn pinned_window_does_not_steal_normal_window_focus() {
    let (mut f, client, _) = window(Config::default());
    let pinned_id = snapshot(&mut f).id;
    f.niri_state().do_action(Action::ToggleWindowPinned, false);
    let surface = {
        let window = f.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    f.roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(200, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let normal_id = f
        .niri()
        .layout
        .windows()
        .find(|(_, w)| w.id().get() != pinned_id)
        .map(|(_, w)| w.id().get())
        .unwrap();
    f.niri_state()
        .do_action(Action::FocusWindow(normal_id), false);
    assert_eq!(f.niri().layout.focus().unwrap().id().get(), normal_id);
    f.niri_state()
        .do_action(Action::FocusWindow(pinned_id), false);
    assert_eq!(f.niri().layout.focus().unwrap().id().get(), pinned_id);
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    assert_eq!(f.niri().layout.focus().unwrap().id().get(), normal_id);
    f.niri_state().ipc_refresh_layout();
    let windows = f.niri().ipc_server.as_ref().unwrap().window_snapshot();
    let hidden = windows.iter().find(|w| w.id == pinned_id).unwrap();
    assert!(hidden.is_hidden);
    assert!(!hidden.is_pinned);
}

#[test]
fn hidden_window_survives_all_outputs_disconnecting() {
    let (mut f, client, _) = window(Config::default());
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    let id = snapshot(&mut f).id;
    let output = f.niri_output(1);
    f.niri().remove_output(&output);
    f.double_roundtrip(client);
    assert!(snapshot(&mut f).is_hidden);
    f.niri_state().do_action(Action::ToggleScratchpad, false);
    assert!(snapshot(&mut f).is_hidden);
    f.add_output(2, (1024, 768));
    f.niri_state()
        .do_action(Action::ScratchpadShowWindow(id), false);
    f.double_roundtrip(client);
    assert!(!snapshot(&mut f).is_hidden);
}

#[test]
fn open_pinned_rule_preserves_open_focused_false() {
    let config = Config::parse_mem(
        r#"
        window-rule {
            match app-id="pinned"
            open-pinned true
            open-focused false
        }
    "#,
    )
    .unwrap();
    let (mut f, client, _) = window(config);
    let original = snapshot(&mut f).id;
    let surface = {
        let window = f.client(client).create_window();
        window.xdg_toplevel.set_app_id("pinned".into());
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    f.roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(200, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    assert_eq!(f.niri().layout.focus().unwrap().id().get(), original);
    f.niri_state().ipc_refresh_layout();
    let windows = f.niri().ipc_server.as_ref().unwrap().window_snapshot();
    let pinned = windows
        .iter()
        .find(|w| w.app_id.as_deref() == Some("pinned"))
        .unwrap();
    assert!(pinned.is_pinned);
    assert!(!pinned.is_focused);
}

#[test]
fn opening_dialogs_for_hidden_and_overlay_windows_is_safe() {
    for hidden in [false, true] {
        let (mut f, client, surface) = window(Config::default());
        if hidden {
            f.niri_state()
                .do_action(Action::MoveWindowToScratchpad, false);
        } else {
            f.niri_state().do_action(Action::ToggleWindowPinned, false);
        }
        let parent = f.client(client).window(&surface).xdg_toplevel.clone();
        let dialog = {
            let window = f.client(client).create_window();
            window.set_parent(Some(&parent));
            let surface = window.surface.clone();
            window.commit();
            surface
        };
        f.roundtrip(client);
        let window = f.client(client).window(&dialog);
        window.attach_new_buffer();
        window.set_size(100, 100);
        window.ack_last_and_commit();
        f.double_roundtrip(client);
        assert_eq!(f.niri().layout.windows().count(), 2);
        f.niri_state().ipc_refresh_layout();
        assert_eq!(
            f.niri()
                .ipc_server
                .as_ref()
                .unwrap()
                .window_snapshot()
                .len(),
            2
        );
    }
}

#[test]
fn fullscreen_windows_show_as_floating_scratchpad_and_pinned_overlays() {
    let (mut f, client, _) = window(Config::default());
    f.niri_state().do_action(Action::FullscreenWindow, false);
    f.double_roundtrip(client);
    f.niri_state()
        .do_action(Action::MoveWindowToScratchpad, false);
    assert!(snapshot(&mut f).is_hidden);
    f.niri_state().do_action(Action::ToggleScratchpad, false);
    f.double_roundtrip(client);
    assert!(snapshot(&mut f).is_floating);

    let config =
        Config::parse_mem("window-rule { open-pinned true; open-fullscreen true; }").unwrap();
    let (mut f, _, _) = window(config);
    let pinned = snapshot(&mut f);
    assert!(pinned.is_pinned);
    assert!(pinned.is_floating);
}

#[test]
fn unpinning_a_detached_window_never_loses_it() {
    let (mut f, client, _) = window(Config::default());
    f.niri_state().do_action(Action::ToggleWindowPinned, false);
    let id = snapshot(&mut f).id;
    let output = f.niri_output(1);
    f.niri().remove_output(&output);
    f.double_roundtrip(client);
    assert_eq!(snapshot(&mut f).id, id);
    f.niri_state()
        .do_action(Action::ToggleWindowPinnedById(id), false);
    let detached = snapshot(&mut f);
    assert_eq!(detached.id, id);
    assert!(!detached.is_pinned);
    f.add_output(2, (1024, 768));
    f.double_roundtrip(client);
    assert_eq!(snapshot(&mut f).id, id);
}
