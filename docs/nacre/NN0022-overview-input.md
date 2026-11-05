# NN0022: native overview input and desk zoom

Collection overview constrained the shown desk's pointer hits and rendered windows to the
scaled workspace background, even when every desk was already shown on an output. Stock niri
allows scrolling and oversized floating windows to extend horizontally beyond that frame.
Those visible portions must remain selectable. With no hidden desk or scratchpad columns,
overview now uses native full-width workspace hit testing and rendering. When side columns
exist, their preview rectangles retain input ownership and the shown desk is clipped to its
own column. Click release still uses the native move grab to select the window and close
overview; dragging continues through the existing desk drop-target path.

Hidden desks no longer divide the remaining output width to choose their zoom. Every desk uses
the configured native overview zoom, preserving its source workspace dimensions. Columns are
placed on either side of the centered shown desk and may extend beyond the output. Workspace
stacks use native physical-pixel size and gap rounding and continue vertically off-screen.
The preview geometry cache includes output scale because that rounding depends on scale.

The input regression includes an oversized floating window outside the workspace frame; it
fails on the original input code. Headless virtual-pointer tests cover selection-changing hover
and click release for shown desks and side previews on two outputs at scales 1 and 1.5. Existing
two-output desk-drop tests continue to cover drag routing. Geometry tests compare collection
overview without side columns with collections disabled at quarter, half, and three-quarter
animation progress, and check native zoom and source sizes for side previews. GPU raster tests
compare incremental and full repaints with and without side columns. Preview pixel-spacing
expectations use the uncropped source transform because visible previews can cross output edges.

Validation and package evidence are recorded in the task's private `.lead/report.md`.
