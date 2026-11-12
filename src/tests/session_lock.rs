use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::backend::DisconnectReason;
use smithay::reexports::wayland_server::{Client as ServerClient, Resource as _};

use super::*;

fn fixture(outputs: u8) -> Fixture {
    let mut f = Fixture::new();
    for n in 1..=outputs {
        f.add_output(n, (800, 600));
    }
    f
}

fn lock_request(f: &mut Fixture, id: super::client::ClientId) -> ExtSessionLockV1 {
    let lock = f.client(id).request_session_lock();
    f.client(id).connection.flush().unwrap();
    f.roundtrip(id);
    lock
}

fn redraw(f: &mut Fixture, output: &Output) {
    let state = f.niri_state();
    state.niri.queue_redraw(output);
    state.niri.redraw(&mut state.backend, output);
}

fn events(f: &mut Fixture, id: super::client::ClientId) -> Vec<ext_session_lock_v1::Event> {
    f.roundtrip(id);
    f.client(id).take_session_lock_events()
}

fn lock_notification_events(
    f: &mut Fixture,
    id: super::client::ClientId,
) -> Vec<super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event> {
    f.roundtrip(id);
    f.client(id).take_lock_notification_events()
}

fn add_restricted_client(f: &mut Fixture) -> super::client::ClientId {
    let (server_stream, client_stream) = UnixStream::pair().unwrap();
    f.niri().insert_client(crate::niri::NewClient {
        client: server_stream,
        restricted: true,
        credentials_unknown: false,
        is_xwayland_satellite: false,
    });
    let client = super::client::Client::new(client_stream);
    let id = client.id;
    let fd = client.event_loop.as_fd().try_clone_to_owned().unwrap();
    f.handle
        .insert_source(
            calloop::generic::Generic::new(fd, calloop::Interest::READ, calloop::Mode::Level),
            move |_, _, state: &mut super::fixture::State| {
                state.client(id).dispatch();
                Ok(calloop::PostAction::Continue)
            },
        )
        .unwrap();
    f.state.clients.push(client);
    f.roundtrip(id);
    id
}

fn assert_locked_event(events: &[ext_session_lock_v1::Event]) {
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ext_session_lock_v1::Event::Locked)),
        "expected the session-lock client to receive locked, got {events:?}"
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, ext_session_lock_v1::Event::Finished)));
}

#[test]
fn refused_client_cannot_unlock_another_clients_session() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let output = f.niri_output(1);
    let owner = lock_request(&mut f, first);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert_locked_event(&events(&mut f, first));
    let owning_lock = match &f.niri().lock_state {
        crate::niri::LockState::Locked(lock) => lock.clone(),
        _ => panic!("the owner must hold an established lock"),
    };

    let refused = lock_request(&mut f, second);
    let refused_events = events(&mut f, second);
    assert!(refused_events
        .iter()
        .any(|event| matches!(event, ext_session_lock_v1::Event::Finished)));
    assert!(!refused_events
        .iter()
        .any(|event| matches!(event, ext_session_lock_v1::Event::Locked)));

    refused.unlock_and_destroy();
    f.client(second).connection.flush().unwrap();
    // The invalid request disconnects B. Do not dispatch its expected protocol error
    // through the fixture's client loop, which treats protocol errors as test failures.
    f.client(second).mark_disconnected();
    dispatch_without_redraw(&mut f);
    assert!(
        matches!(&f.niri().lock_state, crate::niri::LockState::Locked(lock) if lock == &owning_lock),
        "the refused client must leave the established owner locked"
    );

    owner.unlock_and_destroy();
    f.client(first).connection.flush().unwrap();
    f.roundtrip(first);
    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::Unlocked
    ));
}

#[test]
fn skipped_locked_render_keeps_acquisition_locked_and_retries() {
    let mut f = fixture(1);
    let id = f.add_client();
    let output = f.niri_output(1);
    // Install the fault before lock() so it is ready when acquisition enters Locking.
    f.niri_state().niri.test_skip_lock_render = Some(output.clone());
    let _lock = lock_request(&mut f, id);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);

    assert!(f.niri().is_locked());
    let pending = events(&mut f, id);
    assert!(
        !pending.iter().any(|event| matches!(
            event,
            ext_session_lock_v1::Event::Locked | ext_session_lock_v1::Event::Finished
        )),
        "a skipped locked frame must not complete or cancel acquisition: {pending:?}"
    );

    f.niri_state().niri.test_skip_lock_render = None;
    redraw(&mut f, &output);
    assert_locked_event(&events(&mut f, id));
    assert!(f.niri().is_locked());
}

#[test]
fn all_outputs_need_locked_frames_before_locked_event() {
    let mut f = fixture(2);
    let id = f.add_client();
    let output_1 = f.niri_output(1);
    let output_2 = f.niri_output(2);
    f.niri_state().niri.test_skip_lock_render = Some(output_1.clone());
    let _lock = lock_request(&mut f, id);
    f.niri_state().niri.continue_to_locking();

    redraw(&mut f, &output_1);
    redraw(&mut f, &output_2);
    assert!(f.niri().is_locked());
    let pending = events(&mut f, id);
    assert!(
        !pending.iter().any(|event| matches!(
            event,
            ext_session_lock_v1::Event::Locked | ext_session_lock_v1::Event::Finished
        )),
        "one skipped output must keep acquisition pending: {pending:?}"
    );

    f.niri_state().niri.test_skip_lock_render = None;
    redraw(&mut f, &output_1);
    assert_locked_event(&events(&mut f, id));
    assert!(f.niri().is_locked());
}

#[test]
fn lock_notify_waits_for_every_output_and_emits_once() {
    let mut f = fixture(2);
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    let owner = f.add_client();
    let output_1 = f.niri_output(1);
    let output_2 = f.niri_output(2);
    f.niri_state().niri.test_skip_lock_render = Some(output_1.clone());
    let _lock = lock_request(&mut f, owner);
    f.niri_state().niri.continue_to_locking();

    redraw(&mut f, &output_1);
    redraw(&mut f, &output_2);
    assert!(lock_notification_events(&mut f, observer).is_empty());

    f.niri_state().niri.test_skip_lock_render = None;
    redraw(&mut f, &output_1);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event::Locked]
    ));
    redraw(&mut f, &output_2);
    assert!(lock_notification_events(&mut f, observer).is_empty());
}

#[test]
fn lock_notify_reports_lock_unlock_lock_once_each() {
    use super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event;

    let mut f = fixture(1);
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    let owner = f.add_client();
    let output = f.niri_output(1);

    let lock = lock_request(&mut f, owner);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));
    lock.unlock_and_destroy();
    f.client(owner).connection.flush().unwrap();
    f.roundtrip(owner);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Unlocked]
    ));

    let _lock = lock_request(&mut f, owner);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));
}

#[test]
fn lock_notify_late_subscriber_gets_current_locked_state() {
    use super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event;

    let mut f = fixture(1);
    let owner = f.add_client();
    let output = f.niri_output(1);
    let _lock = lock_request(&mut f, owner);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);

    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));
}

#[test]
fn lock_notify_reports_no_output_lock_immediately() {
    use super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event;

    let mut f = fixture(0);
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    let owner = f.add_client();
    let _lock = lock_request(&mut f, owner);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));
}

#[test]
fn lock_notify_global_is_not_advertised_to_restricted_clients() {
    let mut f = fixture(0);
    let unrestricted = f.add_client();
    assert!(f.client(unrestricted).state.lock_notifier.is_some());
    let restricted = add_restricted_client(&mut f);
    assert!(f.client(restricted).state.lock_notifier.is_none());
    assert!(!f
        .client(restricted)
        .state
        .globals
        .iter()
        .any(|global| { global.interface == "hyprland_lock_notifier_v1" }));
}

#[test]
fn removing_failed_output_can_finish_lock_acquisition() {
    let mut f = fixture(2);
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    let id = f.add_client();
    let failed_output = f.niri_output(1);
    let remaining_output = f.niri_output(2);
    f.niri_state().niri.test_skip_lock_render = Some(failed_output.clone());
    let _lock = lock_request(&mut f, id);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &failed_output);
    redraw(&mut f, &remaining_output);
    assert!(!events(&mut f, id)
        .iter()
        .any(|event| matches!(event, ext_session_lock_v1::Event::Locked)));

    f.niri().remove_output(&failed_output);
    assert_locked_event(&events(&mut f, id));
    use super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event;
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));
    assert!(f.niri().is_locked());
}

#[test]
fn destroyed_lock_notification_receives_no_later_events() {
    let mut f = fixture(1);
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    f.client(observer)
        .state
        .lock_notifications
        .last()
        .unwrap()
        .notification
        .destroy();
    f.client(observer).connection.flush().unwrap();
    f.roundtrip(observer);

    let owner = f.add_client();
    let output = f.niri_output(1);
    let _lock = lock_request(&mut f, owner);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert!(lock_notification_events(&mut f, observer).is_empty());
}

#[test]
fn output_added_during_locking_needs_its_own_locked_frame() {
    let mut f = fixture(2);
    let id = f.add_client();
    let output_1 = f.niri_output(1);
    let output_2 = f.niri_output(2);
    f.niri_state().niri.test_skip_lock_render = Some(output_2.clone());
    let _lock = lock_request(&mut f, id);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output_1);
    redraw(&mut f, &output_2);
    assert!(!events(&mut f, id)
        .iter()
        .any(|event| matches!(event, ext_session_lock_v1::Event::Locked)));

    f.add_output(3, (800, 600));
    let output_3 = f.niri_output(3);
    // The fixture's next dispatch may draw queued hotplugged outputs, so hold the new output
    // faulted until after that deterministic dispatch.
    f.niri_state().niri.test_skip_lock_render = Some(output_3.clone());
    redraw(&mut f, &output_2);
    assert!(
        !events(&mut f, id)
            .iter()
            .any(|event| matches!(event, ext_session_lock_v1::Event::Locked)),
        "hotplugged output must render locked content before acquisition completes"
    );
    f.niri_state().niri.test_skip_lock_render = None;
    redraw(&mut f, &output_3);
    assert_locked_event(&events(&mut f, id));
}

#[test]
fn inactive_monitors_count_as_locked() {
    let mut f = fixture(1);
    let id = f.add_client();
    let output = f.niri_output(1);
    f.niri_state().niri.monitors_active = false;
    let _lock = lock_request(&mut f, id);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert_locked_event(&events(&mut f, id));
    assert!(f.niri().is_locked());
}

fn destroy_lock(f: &mut Fixture, id: super::client::ClientId, lock: &ExtSessionLockV1) {
    lock.destroy();
    f.client(id).connection.flush().unwrap();
    dispatch_without_redraw(f);
}

// Process protocol destruction without the fixture's refresh drawing a locked frame first.
fn dispatch_without_redraw(f: &mut Fixture) {
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(std::time::Duration::ZERO, &mut server.state)
        .unwrap();
}

fn takeover_and_confirm(f: &mut Fixture, id: super::client::ClientId, output: &Output) {
    let _replacement = lock_request(f, id);
    assert!(
        f.niri().is_locked(),
        "replacement must inherit the locked session"
    );
    if matches!(
        f.niri().lock_state,
        crate::niri::LockState::WaitingForSurfaces { .. }
    ) {
        f.niri_state().niri.continue_to_locking();
    }
    redraw(f, output);
    assert_locked_event(&events(f, id));
}

fn kill_lock_owner(f: &mut Fixture) {
    let niri = &f.niri_state().niri;
    let client: ServerClient = match &niri.lock_state {
        crate::niri::LockState::WaitingForSurfaces { confirmation, .. }
        | crate::niri::LockState::Locking(confirmation) => {
            confirmation.ext_session_lock().client().unwrap()
        }
        crate::niri::LockState::Locked(lock) => lock.client().unwrap(),
        crate::niri::LockState::Unlocked => panic!("no lock owner"),
    };
    niri.display_handle
        .backend_handle()
        .kill_client(client.id(), DisconnectReason::ConnectionClosed);
}

fn finish_client_death(f: &mut Fixture, id: super::client::ClientId) {
    f.client(id).mark_disconnected();
    dispatch_without_redraw(f);
}

#[test]
fn destroyed_waiting_client_keeps_acquisition_pending_and_can_be_replaced() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let output = f.niri_output(1);
    let lock = lock_request(&mut f, first);

    destroy_lock(&mut f, first, &lock);
    assert!(
        matches!(
            f.niri().lock_state,
            crate::niri::LockState::WaitingForSurfaces { .. }
        ),
        "destroying the lock object must leave acquisition pending"
    );
    takeover_and_confirm(&mut f, second, &output);
}

#[test]
fn destroyed_locking_client_keeps_session_locked_and_can_be_replaced() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let output = f.niri_output(1);
    let lock = lock_request(&mut f, first);
    f.niri_state().niri.continue_to_locking();
    // Hold the queued frame so this test isolates dead-owner replacement from render failure.
    f.niri().output_state.get_mut(&output).unwrap().redraw_state = crate::niri::RedrawState::Idle;

    destroy_lock(&mut f, first, &lock);
    assert!(
        f.niri().is_locked(),
        "destroying the lock object must not unlock the session"
    );
    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::Locking(_)
    ));
    takeover_and_confirm(&mut f, second, &output);
}

#[test]
fn dead_established_lock_can_still_be_taken_over() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let observer = f.add_client();
    f.client(observer).request_lock_notification();
    let output = f.niri_output(1);
    let _lock = lock_request(&mut f, first);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert_locked_event(&events(&mut f, first));
    use super::client::hyprland_lock_notify::v1::client::hyprland_lock_notification_v1::Event;
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Locked]
    ));

    kill_lock_owner(&mut f);
    finish_client_death(&mut f, first);
    assert!(f.niri().is_locked());
    let replacement = lock_request(&mut f, second);
    assert_locked_event(&events(&mut f, second));
    assert!(lock_notification_events(&mut f, observer).is_empty());
    assert!(f.niri().is_locked());
    replacement.unlock_and_destroy();
    f.client(second).connection.flush().unwrap();
    f.roundtrip(second);
    assert!(matches!(
        lock_notification_events(&mut f, observer).as_slice(),
        [Event::Unlocked]
    ));
    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::Unlocked
    ));
}

#[test]
fn dead_waiting_client_keeps_acquisition_pending_and_can_be_replaced() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let output = f.niri_output(1);
    let _lock = lock_request(&mut f, first);

    kill_lock_owner(&mut f);
    finish_client_death(&mut f, first);
    assert!(
        matches!(
            f.niri().lock_state,
            crate::niri::LockState::WaitingForSurfaces { .. }
        ),
        "client death during acquisition must leave takeover possible"
    );
    takeover_and_confirm(&mut f, second, &output);
}

#[test]
fn dead_locking_client_keeps_session_locked_and_can_be_replaced() {
    let mut f = fixture(1);
    let first = f.add_client();
    let second = f.add_client();
    let output = f.niri_output(1);
    let _lock = lock_request(&mut f, first);
    f.niri_state().niri.continue_to_locking();
    // Hold the queued frame so failure here proves stale-owner takeover, not render retry.
    f.niri().output_state.get_mut(&output).unwrap().redraw_state = crate::niri::RedrawState::Idle;

    kill_lock_owner(&mut f);
    finish_client_death(&mut f, first);
    assert!(
        f.niri().is_locked(),
        "client death during acquisition must keep the session locked"
    );
    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::Locking(_)
    ));
    takeover_and_confirm(&mut f, second, &output);
}

#[test]
fn invalid_unlock_callback_during_acquisition_is_ignored() {
    let mut f = fixture(1);
    let id = f.add_client();
    let _lock = lock_request(&mut f, id);
    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::WaitingForSurfaces { .. }
    ));

    <crate::niri::State as smithay::wayland::session_lock::SessionLockHandler>::unlock(
        f.niri_state(),
    );

    assert!(matches!(
        f.niri().lock_state,
        crate::niri::LockState::WaitingForSurfaces { .. }
    ));
    let output = f.niri_output(1);
    f.niri_state().niri.continue_to_locking();
    redraw(&mut f, &output);
    assert!(f.niri().is_locked());
}
