//! NN0036: Caps/Num Lock reach the Nacre event streams once per change.

use niri_ipc::Event;

use super::Fixture;

#[test]
fn lock_state_is_sent_once_per_change_and_only_to_nacre_streams() {
    let mut f = Fixture::new();
    let server = crate::ipc::server::IpcServer::start(&f.niri().event_loop, None).unwrap();
    let nacre = server.subscribe_collection_events();
    let standard = server.subscribe_standard_events();
    f.niri().ipc_server = Some(server);

    // Unchanged state (both off, as in the default snapshot) sends nothing.
    f.niri_state().ipc_refresh_keyboard_lock_state();
    assert!(nacre.try_recv().is_err());

    let keyboard = f.niri().seat.get_keyboard().unwrap();
    let mut mods = keyboard.modifier_state();
    mods.caps_lock = true;
    keyboard.set_modifier_state(mods);
    f.niri_state().ipc_refresh_keyboard_lock_state();
    f.niri_state().ipc_refresh_keyboard_lock_state();
    assert!(matches!(
        nacre.try_recv(),
        Ok(Event::KeyboardLockStateChanged {
            caps_lock: true,
            num_lock: false
        })
    ));
    assert!(nacre.try_recv().is_err());
    assert!(standard.try_recv().is_err());
}
