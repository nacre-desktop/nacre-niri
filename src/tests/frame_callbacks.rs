use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::*;

/// Dispatches the fixture until `done` holds; false if the deadline passed first.
///
/// calloop timers fire from the loop's own timeout, not from an fd, so the server loop is
/// dispatched explicitly each round: the fixture only wakes it for readable fds.
fn wait_until(
    f: &mut Fixture,
    timeout: Duration,
    mut done: impl FnMut(&mut Fixture) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while !done(f) {
        if Instant::now() >= deadline {
            return false;
        }
        f.event_loop
            .dispatch(Duration::from_millis(20), &mut f.state)
            .unwrap();
        f.state.server.dispatch();
    }
    true
}

#[test]
fn fallback_frame_timer_serves_throttled_surface_then_stops() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    assert!(
        f.niri().fallback_frame_timer.is_none(),
        "fallback timer armed before any commit"
    );

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert!(
        f.niri().fallback_frame_timer.is_some(),
        "commit did not arm the fallback timer"
    );

    // The redraw after a commit serves the first callback. The second request lands inside the
    // 995 ms throttle window, so the redraw skips it and only the fallback timer can deliver it.
    let window = f.client(id).window(&surface);
    let first = window.frame();
    window.commit();
    f.double_roundtrip(id);
    assert!(
        first.done.load(Ordering::Relaxed),
        "redraw skipped the first callback"
    );

    let window = f.client(id).window(&surface);
    let second = window.frame();
    window.commit();
    f.double_roundtrip(id);
    assert!(
        !second.done.load(Ordering::Relaxed),
        "throttled callback was sent by the redraw"
    );
    assert!(
        wait_until(&mut f, Duration::from_secs(3), |_| second
            .done
            .load(Ordering::Relaxed)),
        "fallback timer never delivered the throttled callback"
    );

    // Without further commits the timer drops after its next tick and stays off.
    assert!(
        wait_until(&mut f, Duration::from_secs(3), |f| f
            .niri()
            .fallback_frame_timer
            .is_none()),
        "fallback timer kept running with no commits"
    );
    f.event_loop
        .dispatch(Duration::from_millis(1100), &mut f.state)
        .unwrap();
    f.state.server.dispatch();
    assert!(
        f.niri().fallback_frame_timer.is_none(),
        "fallback timer re-armed without a commit"
    );
}
