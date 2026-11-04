# NN0016 compositor reveal rendering

The version-2 extension to `bead_layer_motion_v1` carries the animated contour,
per-corner radii, three-stop rim, optional Gaussian shadow, and independent
content clip alongside the existing plane timeline. Version-1 request opcodes
remain in place. The Bead client uploads content planes once and can retarget a
retained opening from compositor-sampled geometry.

Nacre's quadratic-cut compound contour, rim, and fill currently use a
compositor-owned CPU raster (tiny-skia) uploaded as a cached GLES texture. The
simple circular contour may use Niri's existing rounded-border element; circular
shadows use Niri's rounded-box Gaussian shadow shader with all four sampled
radii. The compositor advances the motion timeline and does not ask Bead to
repaint each refresh. The generic rounded content mask is deliberately absent:
legacy Nacre reveal output clips to the authored rectangular plane clip.

The fixture-backed GLES test compares the complete compositor frame against
Nacre's premultiplied RGBA references for dashboard, launcher, and control
centre at five timeline samples and scales 1 and 1.5, with a one-channel-value
maximum difference. Its atlas and per-frame JSON accompany the images. The
separate ignored cost benchmark reports geometry sampling, contour raster,
texture upload, draw submission, and fenced map/readback separately; run it on
the owner desktop after the host is available. It is not a hardware or battery
measurement, and no live desktop validation has been performed in this change.

Desktop validation remains: run the full nacre-niri CI job, the fixture-backed
GLES test, the 200,000-case layer-motion property suite, and the explicit
`reveal_headless_gles_cost_benchmark`; record GL vendor/renderer/version and
fence-wait separately from draw submission. Also run the Bead and Nacre CI and
route/request-once tests against the final pinned protocol source.
