# NN0029 — fail-closed session-lock acquisition

A required locked frame returning `RenderResult::Skipped` previously called
`unlock()` during acquisition. Dropping Smithay’s unconfirmed `SessionLocker`
sent `finished`, leaving the session unlocked without a client unlock request.
Clasp M0 reproduced this on nacre-niri `1028c5b9`.

## Policy

The surface wait retains upstream behavior: it does not report `is_locked()` or
blank before the client's surfaces are ready. Surface readiness or the one-second
deadline advances acquisition to `Locking`, including when the owner has died.
Neither path drops the unconfirmed locker or returns to `Unlocked`.

During `Locking`, an active-output render failure retains the locker and retries
through one pending 16 ms timer per output. Normal redraw opportunities may
recover sooner; a persistent failure does not create an immediate redraw loop.
Removed outputs cancel their retry timer.

`locked` is sent only when every current output has a locked frame. Hotplugged
outputs must render during acquisition. Upstream’s inactive-monitor rule remains:
with no framebuffer attached, inactive outputs count as locked. Established locks
retain their existing hotplug and dead-owner behavior.

A dead/destroyed acquisition owner can be replaced by a new client without
returning to `Unlocked`. Replacement still needs locked output coverage;
it inherits no authentication. Smithay `ff5fa7df` has no lock-destroy callback,
so replacement checks the stored protocol object’s liveness. Its invalid
pre-readiness unlock request still calls the compositor handler after posting an
error; the handler now refuses to unlock an acquisition.

## Verification

Deterministic Wayland fixture tests inject only a skipped render result through
`cfg(test)`. They drive redraws and the surface deadline directly, without wall
clock sleeps, and inspect client `locked`/`finished` events and compositor state.
The base comparison applies the same tests and test-only hooks to `1028c5b9`.
In the Ubuntu 24.04 CI container:

- `cargo fmt --all` and `cargo test --no-run -j2`: exit 0.
- Candidate: `cargo test -j2 --lib tests::session_lock -- --nocapture`:
  `test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 440 filtered out`.
- Base `1028c5b9`, same tests and test-only hooks, isolated build directory:
  `CARGO_TARGET_DIR=/tmp/nn0029-base-target cargo test -j2 --lib tests::session_lock -- --nocapture`:
  `test result: FAILED. 2 passed; 9 failed; 0 ignored; 0 measured; 440 filtered out`.
  Failures include `skipped_locked_render_keeps_acquisition_locked_and_retries`,
  `all_outputs_need_locked_frames_before_locked_event`, and all four acquisition
  destroy/death takeover cases. The established-death and inactive-monitor tests
  pass on both trees. An earlier base attempt shared a target directory and reused
  a candidate binary; it was discarded and is not acceptance evidence.

`bash /ci/ci-ubuntu.sh` exited 0: debug and release compositor suites each
reported `446 passed; 0 failed; 5 ignored`, with 200,000-case randomized settings
for release. The ignored EGL wheel benchmark, config validation, feature matrix,
profiling and visual-test builds, Rust 1.85 check, Clippy and nightly formatting
passed. Documentation generation passed after installing Git in the disposable
container. The original Clasp M0 takeover proof passed: two outputs, third-output
hotplug, owner death stays locked, replacement requires fresh authentication,
wrong password stays locked, correct password unlocks. No Clasp code was changed.

The Alpine musl build (`cargo build -j2 --no-default-features --features
dbus,xdp-gnome-screencast`), Fedora 42 build (`cargo build -j2 --all`), and FreeBSD
15.1-p6 build (`cargo build -j2 --offline --no-default-features --features
dbus,xdp-gnome-screencast`) passed. FreeBSD used the workflow's pinned PipeWire
patch and verified libdisplay-info 0.3.0 source. The Alpine attempt interrupted by
the shared-host incident was rerun in a fresh container under the new Platform
wrapper limits; the passing rerun is the acceptance result. The optional Nix
`nix --option max-jobs 1 --option cores 2 flake check` ran after correcting
container setup: `426 passed; 6 failed; 3 ignored; 16 filtered out`. All six
failures were EGL/GLES cases reporting “Unable to obtain a valid EGL Display”,
the same environment gap recorded by NN0027.

## Upstream divergence

This compositor-only change is upstreamable; it introduces no shell dependency,
configuration or IPC. Surface-wait presentation remains unchanged. Arch’s stock niri
26.04-1 has the same acquisition flaw: the [v26.04 redraw branch](https://github.com/niri-wm/niri/blob/v26.04/src/niri.rs#L4283-L4304)
also calls `unlock()` on a failed required locked render. The [Arch package](https://archlinux.org/packages/extra/x86_64/niri/)
version was source-checked on 2026-11-11; stock niri was not modified.
