# NN0008: scratchpad and pinned windows

Native scratchpad and pin actions are independently gated by configuration:

```kdl
nacre {
    scratchpad true
    window-pinning true
}
```

Both flags default on in nacre-niri. The `NacreCapabilities` IPC response reports
`scratchpad` and `window_pinning` from the running configuration. Clients must
check these values before emitting fork actions. Stock niri does not understand
these actions or properties.

## Scratchpad

- `move-window-to-scratchpad` hides the focused window; IPC accepts an optional
  window ID. Hidden windows remain alive and can be closed through normal IPC.
- `toggle-scratchpad` shows the most recent scratchpad window as a centered
  floating overlay on the focused output. Repeated use visits older saved windows
  and then hides the overlay; the next press starts again with the most recent.
- `scratchpad-show-window ID` selects a specific saved window.

A dedicated detached workspace reuses the workspace storage and no-output
lifecycle established by collections; it does not implement another tile layout.
Hidden windows are excluded from ordinary workspace views and the recent-window
switcher. The overview provides a scratchpad section. Window IPC exposes
`is_hidden`; it remains false for ordinary detached collection windows.

## Pinning

`toggle-window-pinned` pins the focused window (IPC also accepts an optional ID).
Tiled windows become floating. The output owns pinned windows above its workspace
stack, so switching workspace or collection does not hide them. Window IPC exposes
`is_pinned`. Overlays stay floating; unpin before requesting tiled, fullscreen or
maximized modes. Dragging a window back into the workspace removes its overlay
state. The `open-pinned true` window rule applies this state on mapping,
including initial floating sizing, when the pinning capability is enabled.

## Nacre integration

N0114 emits Super+H (hide), Super+Shift+H (scratchpad), and Super+P (pin) only in
`config/nacre-niri.kdl`, the N0111 overlay. Stock niri configuration retains its
existing behavior. Super+Alt+H retains its existing force-close meaning; hiding a
window is not a substitute for that action. Capability-specific unsupported
metadata is removed only where equivalent behavior is supplied. Native pin rules
for the audio mixer, share picker and picture-in-picture windows also stay in the
fork overlay; its audit is `nacre-niri-unsupported.json`.

## Validation

Acceptance requires layout operations and targeted hide/show, pin/workspace/
collection and hotplug regressions; headless compositor actions and IPC state;
100,000 or more randomized layout cases; compositor tests, format, Clippy and
release build on dev-1; and Nacre generation tests plus its desktop workflow.
Exact results and task commits are recorded in `.lead/report.md` by the task lead.
No installed compositor, live desktop configuration, package or unit is changed.
