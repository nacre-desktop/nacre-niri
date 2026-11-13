# Differences from upstream niri

Base: upstream `v26.04` (merge-base 8ed0da44, 2026-04-25). The `nacre` branch
carries about 310 fork commits on top and merges upstream selectively. Workspace
version `26.4.0`.

Everything not listed here behaves as upstream documents in the
[niri wiki](https://niri-wm.github.io/niri/). Each fork feature has a design
note under [docs/nacre/](README.md); this page is the map.

## Config

New or extended nodes in `config.kdl`:

| Node | Default | Note |
| --- | --- | --- |
| `layout { collections { on; collection "name" {} } }` | on | [NN0001](NN0001-collections.md). `collections { on false; }` restores upstream, including appending unplugged workspaces onto the primary output. |
| `gestures { collection-switch { fingers 4 } }` | on, 4 fingers | swipe between collections |
| `edge-triggers { left|right|top|bottom { action { … }; dwell-time-ms N; pressure N; velocity N; off } }` | none | [NN0010](NN0010-native-widths-edges.md), [NN0019](NN0019-edge-hover-cues.md). Per-output override inside `output "…" { edge-triggers { … } }`. |
| `nacre { overview-hover; scratchpad; window-pinning; proportional-x11-widths; edge-triggers; window-restore; force-close; monitor-workspace-actions; }` | all on | `nacre { name false; }` disables one (`niri-config/src/misc.rs`). |
| `cursor { shake-to-enlarge; }` | off | fork-only |

The shipped `resources/default-config.kdl` also sets Nacre's portable desktop
defaults (6-pixel gaps, 1-pixel focus ring and border, `prefer-no-csd`,
touchpad `scroll-factor 0.75`, `focus-follows-mouse max-scroll-amount="0%"`,
an overview workspace shadow) and the layer rules for `nacre-background` and
`bead-wallpaper*` (placed within the backdrop) and `nacre-topbar` and friends.
These are ordinary upstream config and work on stock niri.

## Bind actions

Added actions you can put in `binds { }`:

| Action | Group |
| --- | --- |
| `focus-collection-next`, `focus-collection-previous`, `focus-collection REF`, `create-collection`, `set-collection-name`, `move-window-to-collection-next`, `move-window-to-collection-previous`, `move-window-to-collection REF` | collections |
| `toggle-scratchpad`, `move-window-to-scratchpad`, `scratchpad-show-window ID`, `toggle-window-pinned` | scratchpad and pinning ([NN0008](NN0008-scratchpad-pin.md)) |
| `restore-last-hidden-window`, `restore-current-workspace-hidden-windows`, `force-close-window` | window restore |
| `swap-workspace-with-monitor-next`, `swap-workspace-with-monitor-previous`, `rotate-workspace-sets-next`, `rotate-workspace-sets-previous` | monitor workspace actions |

Default binds added or changed by the fork: `Mod+Tab` → `toggle-overview`,
`Mod+Ctrl+Tab` / `Mod+Ctrl+Shift+Tab` → next/previous collection, `Mod+Ctrl+N` →
`create-collection`, `Mod+Ctrl+Shift+Page_Down` / `Page_Up` → move window to
next/previous collection, `Mod+M` → `maximize-column` (upstream binds
`maximize-window-to-edges`), `Mod+Shift+WheelScrollDown/Up` → focus column
right/left with a 150 ms cooldown.

## IPC

`niri msg` gains `collections` (the collection registry), `window-under-cursor`,
`event-stream --collections` (adds `CollectionsChanged` and `CollectionActivated`
events) and `nacre-capabilities` (which fork features are active). The
`niri-ipc` crate also adds requests that have no `niri msg` spelling and are
meant for shells: `NacreOverviewHoverStream` and
`NacreOverviewHoverStreamWithCollections` (`NacreOverviewHoverChanged` events),
`NacreEdgeTriggerStream` (`EdgeApproach`, `EdgeHover`, `EdgeTriggered`,
`EdgeLeft`), `EventStreamWithCollections`, and `NacrePrepareLaunch`, which
returns a `NacreLaunchToken` so the launched app lands on the launching
workspace ([launch-workspace.md](launch-workspace.md)). New types: `Collection`,
`Edge`, `NacreCapabilities`.

## Protocols

`resources/` adds `bead-layer-motion-v1.xml` and `bead-layer-promote-v1.xml`
(MIT, from Bead; [NN0006](NN0006-layer-motion.md), [NN0007](NN0007-native-detach.md))
and `hyprland-lock-notify-v1.xml` (BSD-3-Clause; [NN0031](NN0031-lock-notify.md)).

## Behaviour changes without config

- Session lock acquisition fails closed: a lock that cannot be confirmed never
  leaves the session unlocked ([NN0029](NN0029-lock-fail-closed.md),
  [NN0030](NN0030-smithay-session-lock.md)).
- Lock readiness is signalled to `hyprland-lock-notify-v1` clients so hypridle
  can delay suspend until every output is locked and drawn.
- Overview geometry, GPU reveal and input are tuned for a shell drawing over it
  ([NN0012](NN0012-overview-geometry.md), [NN0016](NN0016-gpu-reveal.md),
  [NN0022](NN0022-overview-input.md)).
- Promoted (detached) panels keep pointer priority over layout windows,
  including during layer motion ([NN0027](NN0027-panel-drag.md)).
- Named collections and their window placement persist in
  `~/.local/state/nacre-niri/collections.json`
  ([collection-persistence.md](collection-persistence.md)).

## Packaging

`packaging/arch/PKGBUILD` builds `nacre-niri` pinned to one commit, depends on
stock `niri>=26.04`, installs `/usr/bin/nacre-niri`, the `nacre-niri-session`
wrapper, `nacre-niri.service` user units and the `nacre-niri-fork.desktop`
session entry. The wrapper prefers Nacre's prepared session
(`~/.local/share/nacre/niri-on-pc/session`), then `~/.config/nacre/niri/config.kdl`,
and a `nacre-niri.kdl` overlay beside the chosen config for fork-only actions.
It refuses to start if a graphical session is already running.

## Tracking upstream

The `nacre` branch merges upstream; it does not rebase. The last merged
upstream tag is `v26.04`. The fork's CI mirrors upstream's (`cargo test --all
--exclude niri-visual-tests`, `validate` on the default config, feature-flag
checks) on Ubuntu.
