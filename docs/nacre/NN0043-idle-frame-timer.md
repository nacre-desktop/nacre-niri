# NN0043 — no idle wakeup from the fallback frame-callback timer

Upstream niri runs a 1 Hz timer that sends frame callbacks to every surface
regardless of visibility, throttled to one per 995 ms per surface. It exists so
a client that requested a frame callback on a surface the redraw path will not
reach (occluded, on an inactive workspace, unchanged) still gets one and does
not stall. It also wakes the compositor once a second forever, even with
nothing on screen changing.

nacre-niri arms that timer only after a `wl_surface.commit` (the only way a
frame callback becomes current) and keeps it ticking while commits keep coming.
It drops after the first tick that saw no commit since the previous one: by
then every outstanding callback was either sent by that tick, or had already
been sent by a redraw inside the throttle window, so a callback requested
before the previous tick cannot still be pending. Occluded clients therefore
keep getting callbacks at the upstream rate; an idle compositor has no periodic
wakeup left. Powering monitors back on also arms it, since ticks while they are
off send nothing.

Test: `src/tests/frame_callbacks.rs` (a throttled callback is delivered by the
timer; the timer is unarmed before any commit and drops again afterwards).

Measured on dev-1 (headless labwc parent, nested niri, software rendering,
3 runs each, identical across runs; base f12a6c56 vs this change):

| Case | Base | This change |
| --- | --- | --- |
| Idle, no client, 60 s | main thread 1.12 vol. ctx switches/s, all threads 186 | 0.12/s, 126 |
| Static client (weston-flower) idle, 60 s | 1.10/s, 186 | 0.10/s, 126 |
| weston-simple-shm animating, 15 s | ~145/s; client 49.0 callbacks/s | ~147/s; client 48.9 callbacks/s |
