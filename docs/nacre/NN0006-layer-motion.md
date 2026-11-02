# NN0006: compositor-driven layer motion

## Goal

Let a layer-shell client submit a bounded animation once. Nacre-niri evaluates
it at each output's predicted presentation time and moves/clips existing
textures in the compositor. The client does not commit a buffer for each frame.
The protocol is optional; stock niri keeps the existing client-driven path.

## Protocol contract

Bead defines the generic MIT protocol `bead_layer_motion_v1`; Nacre-niri vendors
that exact XML with its license notice. The manager creates one controller for
a layer-shell root surface. The controller owns ordered planes into one
submitted atlas buffer. Each plane names an immutable source rectangle in
logical surface-texture coordinates after buffer transform and `wp_viewport`
mapping, an initial destination rectangle, a fixed-frame clip rectangle, and
opacity. The client sets each plane's from/target states, one shared easing
curve and opacity interpolation mode, then submits one animation request for
the whole group.
All coordinates use logical surface-local units.

Destination rectangles animate. Clip rectangles remain anchored to the
unanimated root-surface frame and apply after destination placement, letting
content move behind a stationary reveal. A shared root timeline keeps body,
shadow and content planes synchronized. The first created plane is backmost;
each later plane is above earlier planes. Plane creation, target and curve requests apply atomically with the next root-surface
commit; animation begins at the next predicted output presentation after that
commit. Completion emits every plane's final state, then one done event.
Cancellation, replacement and unmap similarly emit final plane states and one
cancellation event. Retargeting samples all planes at the replacement start
time and ignores submitted from values. Stale serials are ignored.

Unmapping or destroying the root cancels safely and drops controller state.
Invalid or non-finite geometry, duration, curve or opacity is rejected or
clamped per protocol contract. No request may create unbounded render geometry.

## Presentation and rendering

Each output samples the shared controller against its predicted presentation
timestamp, so a 120 Hz output gets 120 compositor-side poses without client
commits. Time is monotonic; animation starts at the first presentation after
the armed surface commit. Curves are linear, EaseOut `1-(1-t)^3`, piecewise
EaseInOut cubic, EmphasizedAccelerate `t^4`, and normalized critical spring
`[1-(1+8t)e^(-8t)]/[1-9e^(-8)]`. Per-plane opacity is either linear or
`opaque_above`: after interpolation, alpha above the configured millionths
threshold becomes 1.0. Threshold 1000 preserves Bead's `.001` step behavior.
These planes animate client-buffer presentation only. Layer-shell geometry,
exclusive zones, and compositor-rule shadows/background effects remain at the
layer's configured geometry, matching the existing frame-driven slide path.

The render path composes existing textures with destination transforms,
fixed-frame clips and opacity. It must preserve B0185's split-panel motion: a
single full-panel texture cannot independently resize body/shadow while content
translates behind a growing reveal. The agreed atlas planes encode those parts
and share one timeline. Source/destination aspect mismatch requires
non-uniform texture scaling; otherwise clients should use matching aspect
ratios.

Input hit testing uses the same sampled plane geometry: map points through the
inverse destination transform and reject points outside the fixed-frame clip.
The client may commit its final input region, but hidden pixels must not receive
input during animation. Geometry remains logical until output conversion;
texture sampling and damage must use consistent rounding at fractional scale
1.5.

Damage includes old and new transformed extents and changed clips and flows
through the normal output renderer. Completion restores ordinary layer damage
tracking. Screenshot, screencast and capture paths sample the same pose at the
requested presentation time. Multiple outputs share controller state but use
their own timestamps, scales and damage.

## Correctness and compatibility

Apply the committed start pose before it can be displayed, preserving B0175's
no-open-pose-flash behavior. Keep closing motion visible through its terminal
presentation, preserving B0181's exit semantics and unmap gating. B0178's
client-driven frame scheduling remains intact on the fallback path. The
compositor path changes presentation only, not Bead's semantic open/close
state or submitted pixels.

Tests should sample geometry, clip, opacity and easing; exercise retargeting
and cancellation on unmap; verify dispatch and commit arming; and cover damage
conversion at scale 1.5. Render tests should compare sampled pixels with Bead
goldens. Also validate screenshot/screencast consistency and two outputs.

## Background and status

B0202/B0205 describe battery stutter remaining after B0185's cached slide
content. B0178 defines committed-frame scheduling; B0181 defines close pose
and exit lifetime; B0175 requires the open pose before presentation. The
grouped multi-plane contract is agreed with Bead; server implementation and
exact XML remain in progress. Heavy builds, tests, nested sessions and
captures run only in the assigned dev-1 container.
