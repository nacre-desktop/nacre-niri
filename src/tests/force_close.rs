use niri_config::{Action, Config};
use smithay::reexports::wayland_server::backend::ClientId as ServerClientId;
use smithay::reexports::wayland_server::Resource as _;

use super::Fixture;

fn add_window(f: &mut Fixture, client: super::client::ClientId) -> u64 {
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
    f.niri().layout.focus().unwrap().id().get()
}

fn server_client_id(f: &mut Fixture, id: u64) -> ServerClientId {
    f.niri()
        .layout
        .windows()
        .find(|(_, mapped)| mapped.id().get() == id)
        .unwrap()
        .1
        .toplevel()
        .wl_surface()
        .client()
        .unwrap()
        .id()
}

#[test]
fn force_close_targets_one_wayland_client_and_config_can_disable_it() {
    let mut config = Config::default();
    config.nacre.force_close = false;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 720));
    let first_client = f.add_client();
    let first_id = add_window(&mut f, first_client);
    let sibling_id = add_window(&mut f, first_client);
    let second_client = f.add_client();
    let second_id = add_window(&mut f, second_client);
    let first_server_client = server_client_id(&mut f, first_id);
    let sibling_server_client = server_client_id(&mut f, sibling_id);
    let second_server_client = server_client_id(&mut f, second_id);
    assert_eq!(first_server_client, sibling_server_client);

    f.niri_state()
        .do_action(Action::ForceCloseWindowById(second_id), false);
    assert!(f
        .niri()
        .display_handle
        .backend_handle()
        .get_client_data(first_server_client.clone())
        .is_ok());
    assert!(f
        .niri()
        .display_handle
        .backend_handle()
        .get_client_data(second_server_client.clone())
        .is_ok());

    f.niri().config.borrow_mut().nacre.force_close = true;
    f.niri_state()
        .do_action(Action::ForceCloseWindowById(sibling_id), false);
    let handle = f.niri().display_handle.backend_handle();
    assert!(handle.get_client_data(first_server_client).is_err());
    assert!(handle.get_client_data(second_server_client).is_ok());
}

#[test]
fn force_close_without_focus_and_managed_xwayland_are_noops() {
    let mut f = Fixture::new();
    f.add_output(1, (1280, 720));
    f.niri_state().do_action(Action::ForceCloseWindow, false);

    let client = f.add_client_with_xwayland_satellite(true);
    let id = add_window(&mut f, client);
    f.niri_state()
        .do_action(Action::ForceCloseWindowById(id), false);
    f.roundtrip(client);
    let server_client = server_client_id(&mut f, id);
    assert!(f
        .niri()
        .display_handle
        .backend_handle()
        .get_client_data(server_client)
        .is_ok());
}
