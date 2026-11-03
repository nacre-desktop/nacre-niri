# NN0007: native layer promotion (pearls)

## Ownership and compatibility

The optional MIT Bead protocol `bead_layer_promote_v1` promotes an existing
layer-shell root into compositor-owned floating presentation. Its `wl_surface`,
role, current buffer tree, motion controller and render identities stay intact.
This is an internal presentation change, not an attempted reassignment of the
Wayland role to xdg-shell. Bead discovers the manager global and retains its
prepared-xdg implementation on compositors without it.

A controller stages promote, dock or cancel until the root commit. Rendering
and input consult the same promotion map: an entry suppresses ordinary layer
rendering and hit testing, including while its workspace is hidden. Only the
floating path draws it. There is no precreated hidden window, independent cover
surface, opacity bootstrap or two-buffer handoff. Normal output damage tracking
invalidates the previous placement when the same render identities move.

The body rectangle is in root-local logical coordinates; it may occupy only
part of the root texture. Placement starts at the compositor's current layer
geometry plus the body's offset. The full root content remains available for
client-rendered edges outside that geometry. Native presentation has no layer
opacity, bob, blur or shadow policy. Capture blocking remains respected.
Ordinary client popups retain their popup policy.

After promotion the client can render its natural extent on the same root,
even when layer arrangement had clipped the docked buffer. `set_body_size`
stages new logical body dimensions with that content commit. The body origin,
pointer hotspot and promotion serial stay unchanged; the new dimensions must
fit the newly committed texture. The client retains the layer's configured
dimensions so docking restores its original clip. No configure roundtrip or
new role is needed during the held grab.

## Input, outputs and lifetime

Interactive promotion validates the seat and current pointer-button serial
against the root or one of its subsurfaces. A dedicated grab preserves the
original hotspot, includes travel since the press, and continues delivering
motion, relative motion and release to the client. Command promotion uses a
null seat and zero button serial. Stale request serials are ignored using
wraparound comparison. Expired grabs reject without changing ownership.
The standard close-window action targets a focused promotion through its
existing layer-shell close event, preserving the layout window below it.

The compositor owns position while promoted. Crossing an output transfers the
same layer tree between output maps and sends preferred scale/transform updates.
Output removal migrates to a surviving output, or closes and retires promotion
when no outputs remain. A promoted surface is associated with its workspace;
hidden workspaces and collections cannot render or receive input. This does
not insert a fake xdg window into the layout or change collection IPC.
If layout cleanup reclaims an otherwise empty workspace, promotion follows the
active workspace within the same collection; it never follows a different
collection merely because that collection occupies the old output.
Controller creation validates a live layer role rather than output membership.
During startup or output loss, the client may already have queued motion and
promotion controller requests before receiving the layer's close event. Both
requests remain harmless; a missing output does not disconnect the client.

Dock restores ordinary layer ownership on the current output; cancel prefers
the original output if it survives. The retained root returns to its dock using
`LayerMotion` and its EaseOut cubic sampler, sampled at presentation time. Input
uses the same translation. Exclusive zones are suspended during promotion and
restored on docking. Null-buffer commits retire promotion but retain a reusable
controller. Controller or role destruction retires its ownership and grab.

Nacre's native session generation omits the pearl pending-window and decoration
rules. Stock niri retains them. B0200's prepared target placement, B0209's cover
lifetime and N0103's pearl-specific window-rule ordering are unnecessary for the
native path; the independent Widgets rule remains.

## Verification

`src/tests/layer_promote.rs` exercises protocol commit staging, same-surface
identity, promotion, immediate interactive movement, release, docking,
cancellation, stale serials, invalid body rejection, output migration,
monitorless retirement and hidden collections. Its fractional-scale GLES test
compares the actual output handoff frame and incremental damage repaint against
a fresh full raster, including removal of old-location pixels.

Build/test evidence and the three task branch revisions are recorded in the
unit handoff report. Source and raster tests do not claim a live desktop audit.
