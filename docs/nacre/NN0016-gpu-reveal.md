# NN0016 compositor reveal rendering

The version-2 extension to `bead_layer_motion_v1` carries the animated contour,
per-corner radii, three-stop rim, optional Gaussian shadow, and independent
content clip alongside the existing plane timeline. Version-1 request opcodes
remain in place. The Bead client uploads content planes once and can retarget a
retained opening from compositor-sampled geometry.

Nacre's quadratic-cut compound contour, rim, and fill use compositor-owned
tiny-skia rasters uploaded to GLES. Large constant-radius one-axis grows can
cache a full-open contour and a validated 52-logical-pixel ghost edge band,
then move the band by the same rounded physical-pixel shift as Bead's MIT
`TreeCache` (`crates/bead-render/src/tree.rs` at commit
`0eb9a9a8afdbd8e8a2f6770e1a3b05818af198b0`). The complete upstream MIT notice
is retained in [`licenses/Bead-MIT.txt`](licenses/Bead-MIT.txt). This preserves
the baked rim-gradient phase and avoids per-frame
contour raster/upload on eligible frames. The retained texture pair is capped
at 64 MiB per layer. Each tiny-skia raster pass also holds separate fill and rim
pixmaps, each subject to the existing 64 MiB per-image guard, and applies the
SVG group's opacity when compositing the rim layer. Unsupported axes/styles,
failed validation, or oversize pairs use the existing single-pose raster
fallback. Early one-axis grows use Bead's MIT `sliced_art` method when the
16-logical-pixel center strip is uniform within four channel values: render
96-logical-pixel end slices and stretch the center line, keeping the legacy
gradient phase. Tiny-skia still builds initial templates synchronously on the
first eligible frame. Circular shadows use Niri's rounded-box Gaussian shadow shader
with all four sampled radii. The compositor advances the motion timeline and
does not ask Bead to repaint each refresh. The generic rounded content mask is
deliberately absent: legacy Nacre reveal output clips to the authored
rectangular plane clip.

The fixture-backed GLES test compares the complete compositor frame against
Nacre's premultiplied RGBA references for dashboard, launcher, and control
centre at five timeline samples and scales 1 and 1.5, with a one-channel-value
maximum difference. Its atlas and per-frame JSON accompany the images. The
separate ignored cost benchmark renders through `MappedLayer::render_normal`,
reporting element-build/raster/upload, draw submission, and fence wait
separately. It uses the same sampled plane clips and viewport path as the full
frame comparator. It does not read back pixels, and allocates a headless render
target per sampled frame; run it on the owner desktop after the host is
available. Record the template's one-time bake frame when it occurs; that bake
is part of active element-build timing. It is not a battery measurement, and no
live desktop validation has been performed in this change.

Desktop validation remains: run the full nacre-niri CI job, the fixture-backed
GLES test, the 200,000-case layer-motion property suite, and the explicit
release-mode `reveal_headless_gles_cost_benchmark`; record `eglinfo -B` vendor,
renderer, and version alongside the separate draw-submit and fence-wait
results. Also run the Bead and Nacre CI and route/request-once tests against the
final pinned protocol source.
