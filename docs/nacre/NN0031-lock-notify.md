# NN0031 — lock readiness notifications for suspend

hypridle 0.1.8 normally releases its logind sleep delay inhibitor immediately
after spawning `before_sleep_cmd`. That can let suspend overtake lock rendering.
nacre-niri now exposes `hyprland-lock-notify-v1` to unrestricted clients, allowing
hypridle to retain the inhibitor until the compositor reports the lock ready.

The protocol XML is vendored unchanged from hyprland-protocols commit
`32a0b11b6fb1bbca4391e2f764f013394d4ff5a8`; its license is retained in
[`licenses/hyprland-protocols-BSD-3-Clause.txt`](licenses/hyprland-protocols-BSD-3-Clause.txt).

`locked` follows the same confirmation as ext-session-lock: every remaining
output must have a successful locked frame. Removing an output or having no
outputs can complete acquisition. Waiting for surfaces and failed renders do
not notify. Replacing a dead lock owner preserves the established locked status
without a duplicate event. A subscriber created while fully locked receives
`locked` immediately. An actual unlock after full locking sends one `unlocked`.

The owner's hypridle configuration needs **no change**: default
`inhibit_sleep = 2` detects this protocol with
`before_sleep_cmd = loginctl lock-session` and `lock_cmd = hyprlock`.
logind's `InhibitDelayMaxSec` still bounds the delay (normally five seconds),
so lock readiness must fit inside that limit.

The deterministic fixture covers every-output readiness (including a skipped
render), exact lock/unlock/lock ordering, late subscriptions, no outputs,
output removal, dead-owner takeover, destroyed subscriptions, and restricted
clients. In a fresh Ubuntu 24.04 container on dev-1,
`cargo test --locked -j2 --lib tests::session_lock -- --nocapture` passed all
18 tests (the existing 12 plus six new tests). `cargo test --locked -j2 --lib`
passed 453 tests with five ignored; Clippy and nightly rustfmt checks passed.

With the same test client and XML on unpatched `f8a95193`, the tests compile but
fail eight cases because the notifier global is absent (10 pass). The candidate
release build also passes.

## S3 ordering proof

On dev-1, a copied QEMU q35 guest ran an active seat0/tty1 session with
hypridle 0.1.8-2 and the configuration above (`inhibit_sleep` unset). Its
startup reported inhibition until the Wayland session locks, and logind listed
hypridle's sleep delay inhibitor. The measured delay limit was 5.000 seconds.

The counted profile used standard VGA, Weston headless Pixman, and Niri's
1280×800 Winit output with software EGL. Direct virtio-GPU trials stalled after
resume and were excluded. Both revisions used the same static hyprlock 0.9.7-2
test configuration (animations disabled) and one-second locker startup delay.
These are private harness settings, not changes required in the owner's config.

In one guest `journalctl -b -o short-monotonic` clock, the canonical readiness
marker was hyprlock's actual `ext_session_lock_v1.locked()` event. All three
unpatched `f8a95193` controls entered suspend before that event. All **20/20**
candidate cycles had `locked < PM: suspend entry < S3 wake < PM: suspend exit
< unlock_and_destroy`, including real ACPI S3 wake markers. Request-to-locked
latency was 1.087–1.166 seconds; locker-start-to-locked was 1.063–1.142 seconds.
The monotonic journal clock excludes suspended time; these are readiness
latencies, not measurements of suspend duration.

After the completed proof, guest poweroff panicked during shutdown; the test VM
was stopped through its QEMU monitor. No owner session was changed.
