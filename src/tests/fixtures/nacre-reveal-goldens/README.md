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
`88cb0f2f`, using Bead's final REVEAL opening planner at commit
`0eb9a9a8afdbd8e8a2f6770e1a3b05818af198b0` (source SHA-256
`88de30ead7a1dd484a6ce0f20a7dd911d665e7736b5e76020c3bf62a4bc3c472`). The
vendored protocol XML is Bead's canonical source at that commit (SHA-256
`855cd28c2aba1ef29289787c1014719ca4438c77eb2045f205e7cd2d79a2c283`).
For REVEAL openings with no explicit distance, the content plane stays at its
open position (zero offset); the contour body still grows from the bar. The
fixed export regenerates the atlas, plans, and all 30 full-canvas CPU samples.
