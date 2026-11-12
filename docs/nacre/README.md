# Nacre Niri documentation

These documents describe fork-specific behavior and integration contracts. They supplement the [upstream niri documentation](https://niri-wm.github.io/niri/) and are not general compositor setup guidance.

## Current contracts

- [Workspace collections](NN0001-collections.md) and [collection persistence](collection-persistence.md)
- [Layer motion](NN0006-layer-motion.md) and [native layer detach](NN0007-native-detach.md)
- [Scratchpad pinning](NN0008-scratchpad-pin.md)
- [Native widths and edge cues](NN0010-native-widths-edges.md) and [edge hover cues](NN0019-edge-hover-cues.md)
- [Overview geometry](NN0012-overview-geometry.md), [GPU reveal](NN0016-gpu-reveal.md), and [overview input](NN0022-overview-input.md)
- [Fail-closed session-lock acquisition](NN0029-lock-fail-closed.md)
- [Session-lock owner checks from upstream Smithay](NN0030-smithay-session-lock.md)

## Historical and integration notes

- [Nacre integration boundary](NN0002-nacre-integration.md) records the shell/compositor integration scope.
- [Launch workspace affinity](launch-workspace.md) records NN0025's bounded launch tracking and placement policy.
- [Workaround sweep](NN0017-workaround-sweep.md) is a dated historical snapshot; consult the current contracts above for supported behavior.
