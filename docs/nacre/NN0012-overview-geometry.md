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

The old render preparation visited every collection for every output and then
prepared the output's normal workspaces again. Shown collections now prepare and
render through their native monitor. Hidden workspace geometry and GLES snapshots
are cached. Source commits, layout changes, urgency, animation, capture target,
output scale, renderer context, and wallpaper damage invalidate the relevant cache.
Snapshot placement also changes wallpaper sampling, so opening animation frames
can rebuild snapshots; unchanged steady frames reuse them. Hidden snapshots follow
surface commits with offscreen frame-callback throttling.

The headless regression inspects final elements and output damage at quarter
progress on 1920×1200 logical at scale 1.5 and 3440×1440 at scale 1.0. The standalone
benchmark measures changing opening frames and steady frames separately, with
element preparation/assembly separated from GLES drawing and synchronous readback.
Readback adds overhead; these timings support comparison and do not measure physical
display presentation. Results and exit codes belong in the task's private report.
No installed compositor, live configuration, or live session is changed.
