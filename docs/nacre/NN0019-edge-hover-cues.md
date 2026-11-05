# NN0019: Native edge hover cues

Native edge triggers can emit pre-dwell pointer cues on the dedicated
`NacreEdgeTriggerStream`. `EdgeApproach` marks entry into an enabled event edge
band. `EdgeHover` reports output-local pointer position and dwell progress from
0 to 1 at no more than the output refresh rate. `EdgeTriggered` and `EdgeLeft`
retain their existing meanings. The ordinary `EventStream` never receives edge
events.

Cues are opt-in through the global or per-output `cue` flag in an
`edge-triggers` block. They are available only for event-action edges. The
`trigger-distance` field sets the inward logical-pixel width of one edge's
activation band; its value is half-open at the inner boundary (`10` accepts
positions less than 10 pixels from the left/top or at least output-size minus
10 pixels from the right/bottom). An omitted value preserves the existing
one-pixel native edge band. Nacre's generated configuration sets this width to
the matching frame-lip thickness.

The optional `trigger-length` field limits activation to a centered span along
the output edge. Its inner bound is half-open, and a span longer than the output
is clipped to the output size. Omitted values keep the full-edge activation
behavior. Nacre's generated configuration uses `144` logical pixels for the
side lips and `208` for the top lip by default.

```kdl
edge-triggers {
    cue
    left { action { event; }; trigger-distance 10; trigger-length 144; dwell-time-ms 350; }
    right { action { event; }; trigger-distance 10; trigger-length 144; dwell-time-ms 350; }
    top { action { event; }; trigger-distance 10; trigger-length 208; dwell-time-ms 350; }
}

output "DP-1" {
    edge-triggers { cue false; }
}
```

The `edge_trigger_cues` Nacre capability is true only when at least one
effective, enabled event edge has cues enabled. Shells should require that
capability before replacing stock hover surfaces; `edge_triggers` alone only
promises completed trigger events.
