<!-- SPDX-License-Identifier: GPL-3.0-only -->

# Nacre reveal CPU references

These are the current Nacre CPU-rendered frames for the dashboard, launcher,
and control-centre openings. The route smoke test compares each frame byte for
byte with `TreeCpuRenderer` output and exports the same set when
`NACRE_REVEAL_REFERENCE_DIR` is set.

Each route has samples at elapsed progress 0, 0.25, 0.5, 0.75, and 1 at scales 1 and
1.5. PNG files contain the full layer-surface buffer canvas (origin 0,0) as
premultiplied RGBA8 channel bytes. Each per-frame JSON also records scene bounds,
scene offset, the layer surface's output-global origin, contour crop coordinates,
elapsed sample time, and eased progress. Each route/scale also has
a `-plan.json` file containing the exact `LayerMotionPlan` plane tuples and
root-local reveal descriptor sent toward Bead, plus a `-atlas.png` image of the
plan's content atlas. Nacre-Niri uses those files and the full-root PNGs to
compare compositor output against the existing `RevealArt.draw` result; these
are not screenshots from a live compositor.


Provenance: exported from the Nacre route fixture test in worktree base
`88cb0f2f`, using Bead's REVEAL opening planner fix source SHA-256
`af28cf108cfb7afb211463fdfb5f8ebe64a371145ae89c8b497495a127705638`.
For REVEAL openings with no explicit distance, the content plane stays at its
open position (zero offset); the contour body still grows from the bar. The
fixed export regenerates the atlas, plans, and all 30 full-canvas CPU samples.
