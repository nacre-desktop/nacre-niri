# NN0025: Launch workspace affinity

Nacre native launchers can register an application launch with nacre-niri before
starting it. The compositor captures the current workspace and returns a real
xdg-activation token. The launcher passes that value as
`XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`; the IPC token can also be bound
to the transient app unit without changing its original workspace or 60-second
deadline.

At map time, the first eligible unparented toplevel consumes the still-valid
launch record. Explicit `open-on-workspace` and `open-on-output` rules remain authoritative,
parented dialogs keep their parent placement, and launch intent takes precedence
over automatic saved app routing. A window placed off the active workspace does
not focus or switch the view, including with `open-focused true`; it is marked
urgent. Late activation for that same mapped surface stays urgent-only while its
launch token remains valid, including when a rule chooses its off-view
destination. A second launch that activates an already mapped window retires the
unused record and retains normal activation behavior. A later independent
window from an already mapped native client process receives today's placement.

Records expire at the 60-second boundary, and the registry is capped at 128.
The feature's declared additional memory budget is at most 1 MiB for those
bounded records; no separate runtime memory measurement was taken.
Niri `spawn` and `spawn-sh` actions capture the workspace and pass the registered
token. A standards-aware client with an accepted serial-bearing xdg-activation
token also records the current workspace; urgency-only and rejected tokens do
not. If the native client does not request protocol activation, the compositor
checks each recognized inherited startup-token environment value, then its
exact cgroup unit.
Native Nacre launchers can bind the known `nacre-app-*.service` unit, covering
clients that strip the token. Niri's spawn actions also register the actual
grandchild PID before releasing it to exec; PID plus `/proc` start time provides
a bounded ancestry fallback when a wrapper strips the token and changes cgroup.
The wait fails open after three seconds if the compositor cannot acknowledge.
Tokenless X11 apps cannot use the native PID fallback because the compositor
sees the satellite PID; they need the satellite's activation-token bridge. On
the desktop TEST host, a delayed one-window X11 client through
xwayland-satellite 0.8.3 preserved its launch workspace after 12 seconds.
GTK/Zenity still opened on the current workspace and focused there. X11
coverage is therefore partial; the cause is unconfirmed, with an early
satellite leader window consuming the token as one possibility.

The QML-only legacy launch path is not instrumented because the QML shell is
being retired. This feature does not alter persisted app routes or create
workspaces that have disappeared before map.

## Validation

On dev-1, `nice -n 10 cargo test -j2 --all --exclude niri-visual-tests launch -- --nocapture` passed 13 tests, including the exact 60-second expiry boundary. The full Ubuntu 24.04 workflow (`.github/workflows/ci.yml`) completed with exit 0: debug and release suites each passed 433 tests with 5 ignored; the ignored EGL benchmark, config validation, feature builds, visual-test build, Rust 1.85 check, Clippy, and nightly rustfmt also passed.

The private desktop nested check passed 10 cases, including delayed native, spawn, PID-ancestry, exact-unit, stale-token fallback, and minimal X11 launches; every passing case mapped on the launch workspace while the view and focus stayed on the switched workspace. GTK/Zenity X11 still mapped and focused on the current workspace, so delayed X11 coverage remains partial.

The Alpine musl build command was `nice -n 10 cargo build -j2 --no-default-features --features dbus,xdp-gnome-screencast`; Fedora ran `nice -n 10 cargo build -j2 --all`. Both exited 0.

The optional Nix flake check reports the same six EGL-display failures on both this source and clean `origin/nacre` baseline `feaa5c1`: 413 passed, 6 failed, 3 ignored with NN0025, and 400 passed, 6 failed, 3 ignored on baseline. The failing layer-motion and layer-promote tests cannot obtain an EGL display in the Nix sandbox; the flake skips tests matching `::egl`, which these tests do not match.

The FreeBSD 15.0 workflow could not build with the current package repository. The pinned guest was `15.0-RELEASE-p13`; the repository supplied newer FreeBSD 15 packages. With `IGNORE_OSVERSION=yes`, package installation completed, but its Rust binary required `FBSD_1.9`, which is absent from the guest libc. No guest upgrade or product change was made.
