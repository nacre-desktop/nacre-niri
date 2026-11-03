# NN0012: overview geometry and preview cost

The r2847 collection overview replaced the monitor's workspace-window renderer
with its own collection-column renderer. The surrounding render path still drew
workspace layers, backgrounds, and shadows using native monitor geometry. Thus
the frame and its windows could have different origins, sizes, and animation
positions. The hover camera was also bypassed by the replacement window path.

The old path emitted the shown workspace background twice with the same render
element identity at two different positions. Damage tracking expects each identity
to describe one placement in an output frame; this duplicate is a concrete cause
of unreliable background damage. The owner's reported garbage is consistent with
this defect; physical display tearing is not established by a headless test.

Each output now keeps its shown collection on the native workspace path. Only
hidden and detached collections use side previews, with one transform for their
windows, background, and shadow. Collections shown on other connected outputs
are excluded: the other monitor already presents them. Side columns occupy the
space remaining beside the native overview, preserving its zoom and camera.

Validation and measured performance are recorded in the task's private evidence
and report. No installed compositor, live configuration, or live session is changed.
