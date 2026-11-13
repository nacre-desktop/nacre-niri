# NN0041: Sticky inner monitor edges (inner-edge push)

On the edge between two monitors the pointer just passes through, so an edge
trigger configured there (the Nacre side-panel lip) never gets its dwell. With
`inner-edge-push` (on by default) the compositor holds relative pointer motion
at such an edge while the edge's ordinary trigger runs, and lets it through
when the push is clearly a move to the other monitor. The shell needs nothing
new: the hold drives the existing `EdgeTriggered`/`EdgeLeft` (and
`EdgeApproach`/`EdgeHover` with `cue`) events on `NacreEdgeTriggerStream`.

```kdl
edge-triggers {
    inner-edge-push {
        // off
        hold-time-ms 180
        cross-velocity 1500
        cross-distance 64
        left true; right true; top true; bottom true
    }
    right { action { event; }; dwell-time-ms 350; }
}
output "DP-1" { edge-triggers { inner-edge-push { right false; }; }; }
```

An edge is sticky when all of these hold: `nacre { edge-triggers }` is on, the
edge has an enabled trigger (any action) and the pointer is inside its
`trigger-length` span if one is set, `inner-edge-push` is not `off` and the edge
is enabled in it, and another output lies straight across the edge at the
pointer's along-edge coordinate. The per-output block replaces the global one
as a unit. Absolute motion (tablets, touch), pointer confinement, spatial-grab
wrapping, the overview, fullscreen, lock and grabs are never held.

## Feel and defaults

Each held event adds its inward motion to a push value that decays at
250 px/s; the pointer crosses when the push reaches `cross-distance` or a single
event's inward speed reaches `cross-velocity`. The hold time replaces the edge's
`dwell-time-ms` on inner edges only (the pointer is physically stopped, so the
user feels the hold; 180 ms is short enough to feel immediate and long enough
that a bump during a move does not open a panel).

| Inward speed | What happens |
| --- | --- |
| at or above 1500 px/s (a normal flick to the other monitor) | crosses on the first event with velocity history, i.e. within one input report |
| 800 to 1400 px/s (a casual move) | a 55 to 115 ms bump at the edge, then crosses; no trigger, because 64 px of net push arrive before 180 ms |
| around 600 px/s | the boundary: crosses at about the hold time |
| below 500 px/s (leaning on the edge) | held; the trigger fires after 180 ms; crossing then needs a sustained push (300 px/s crosses after 1.3 s, 100 px/s never) |
| stopping for a second | push decays to zero; the next approach starts fresh |

Velocity uses the input event timestamps, not wall-clock time, so events that
arrive batched after a busy frame are not mistaken for a fling. The first held
event has no velocity history and is always held once; the cost is one input
report, so a fling is never blocked for more than a few milliseconds. The decay
constant is fixed in `src/edge_triggers.rs` (`PUSH_DECAY_PX_PER_S`).

## Output adjacency over IPC

Collection streams (`EventStreamWithCollections`,
`NacreOverviewHoverStreamWithCollections`, `niri msg event-stream --collections`)
carry `OutputNeighboursChanged { neighbours }` in the snapshot and on every
change (hotplug, removal, position, mode or scale change). Each entry is
`{ output, edge, neighbour, start, end }`: the neighbour touching `edge` of
`output`, with the shared half-open segment `[start, end)` in `output`'s local
logical coordinates (y for left/right, x for top/bottom). Both directions are
listed, and a 1 px gap is not adjacent. Edges without an entry are outer edges.
Plain `EventStream` clients never see it. Geometry is the rounded logical layout
used for pointer clamping, so a scaled neighbour that overlaps partly yields a
sub-span.

The `inner_edge_push` capability is true when the native edge-trigger flag is on
and `inner-edge-push` is not `off`. Stock niri and Hyprland report false; the
shell falls back to bar buttons there.
