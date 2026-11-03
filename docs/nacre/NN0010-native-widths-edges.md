# NN0010: native proportional X11 widths and screen edges

The fork enables both policies by default. Disable them independently with
`nacre { proportional-x11-widths false; edge-triggers false; }`.
The bundled fallback does not configure any edge actions.

## Width policy

The managed xwayland-satellite connects through a separate Wayland listening
socket. Only clients accepted there receive the satellite marker; app IDs and
titles are not used to classify native Wayland windows. The satellite receives
the private `WAYLAND_DISPLAY`, with `WAYLAND_SOCKET` removed, so its Xwayland
child can still use the satellite's own internal display.

Initial and mapped tiled sizing treat an adjustable horizontal minimum greater
than one as one logical pixel. Height and maximum constraints remain effective;
fixed or contradictory minimum/maximum widths are preserved. Parent/transient
windows are excluded. Floating and policy-disabled windows see the original
constraints. No X11 property or cached XDG constraint is rewritten. Reload
updates policy on windows in every workspace, including detached collections,
before recalculating layout.

Satellite forwards XDG minimum/maximum sizes, but not `_NET_WM_WINDOW_TYPE`.
Consequently, an unparented non-normal X11 toplevel receives this default policy
too. X11 increment/aspect properties remain unchanged; this does not add support
for constraints that satellite does not forward. An externally started satellite
does not receive the marker. `proportional_x11_widths` capability is true only
when the policy and managed satellite integration are active; otherwise the
Nacre compatibility helper remains necessary.

## Edges

```kdl
edge-triggers {
    left { action { event; }; dwell-time-ms 350; }
    right { action { spawn "panel-command" "right"; }; pressure 12; velocity 600; }
    top { action { spawn-sh "panel-command top"; }; }
}
output "DP-1" {
    edge-triggers { left { off; }; }
}
```

Individual output edges override global edge definitions. `dwell-time-ms`
defaults to 250; pressure and velocity default to zero. Pressure is movement
toward the edge in logical pixels, and velocity is logical pixels per second.
Once both gates pass, the pointer must remain at that edge for the dwell time.
A fired edge stays latched until the pointer leaves it. Geometry uses the same
rounded logical output bounds as pointer clamping, including fractional scale.

Overview, fullscreen, lock, and outputs without a layout monitor suppress
actions. Pointer grabs, confinement, and spatial wrapping suppress edge input.
Reload and output geometry/removal cancel pending triggers. No lookup
assumes that a workspace has a monitor.

The `edge_triggers` capability reflects the native flag. Subscribe using JSON
`"NacreEdgeTriggerStream"`; after `{"Ok":"Handled"}`, the opt-in stream can
contain `{"EdgeTriggered":{"output":"DP-1","edge":"Left","position":[0.0,600.0]}}`.
Positions are output-local logical coordinates. Existing event streams never
receive the new event. Consumers must filter the edge event from ordinary stream
snapshots/events, and restore their compatibility input source on disconnect.
Disabling the native flag on reload disconnects only edge subscribers. Consumers
can query capabilities and reconnect after `ConfigLoaded` when the flag returns.
The event reports a completed trigger; it does not expose pre-trigger dwell
progress for client-side hover cues.

Nacre owns panel selection, lip hit testing, dwell settings, and generated edge
actions. Fork-only configuration belongs in its sibling `nacre-niri.kdl` overlay;
stock niri configurations never include it. Native source takeover must follow
an accepted IPC stream, rather than binary identity alone.

## Validation

The implementation passed in an isolated Ubuntu 24.04 container with 8 CPUs,
16 GiB, and two Cargo jobs. All 5,499 checked source/resource files matched the
tested snapshot. Every check below returned zero:

- Workspace tests/doc tests: 279 passed; the one opt-in X11 proof also passed.
- Real managed satellite (`7bf5e0dcb0d4250860b24b04a6cb3ddeada515d0`), Xwayland,
  and Xlib proof at scale 1.5: tiled/floating/config-off geometry and unchanged
  raw minimum, maximum, height, increment, and aspect hints.
- Collection operation and edge geometry properties: 100,000 cases each;
  the width policy property also runs 100,000 cases in the normal suite.
- Nightly formatting, Clippy, bundled config validation, no-default/dbus/systemd/
  dinit/screencast feature checks, profiling build, and visual-test compilation.

Clippy retains four inherited warnings and adds none. Rendered interaction and
live desktop acceptance were not run; no installed runtime or live config changed.
