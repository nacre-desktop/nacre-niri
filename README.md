# nacre-niri

Nacre's fork of the [niri](https://github.com/niri-wm/niri) scrollable-tiling
Wayland compositor. It is niri 26.04 plus the compositor-side features the Nacre
desktop needs: workspace collections, a scratchpad and pinned windows, screen-edge
triggers, native panel motion, an overview that reports hover, and a lock that
fails closed. Most additions have a config switch; turning one off restores
upstream behaviour. It is not a desktop shell; the Bead shell is maintained in
the [Nacre repository](https://github.com/nacre-desktop/nacre).

<!-- media: media/nacre-niri-tabbed-column.png — a tabbed column (Mod+G) with three windows; overview behind showing two collections -->

You can run it without Nacre. It installs beside stock niri and reads the same
config format.

## What the fork adds

| Feature | Config | Bindings and IPC |
| --- | --- | --- |
| **Workspace collections**: stacks of workspaces with a stable identity independent of their output. Unplug a monitor and its collection waits; plug it back and it returns. | `layout { collections { on; collection "work" {} } }` (on by default) | `focus-collection-next/previous`, `focus-collection`, `create-collection`, `set-collection-name`, `move-window-to-collection[-next/-previous]`; `niri msg collections` |
| **Scratchpad and pinning** | `nacre { scratchpad; window-pinning; }` | `toggle-scratchpad`, `move-window-to-scratchpad`, `scratchpad-show-window ID`, `toggle-window-pinned` |
| **Edge triggers**: dwell, pressure or velocity on a screen edge fires an action. Nacre uses them for its side panels. | `edge-triggers { left { action { … }; dwell-time-ms 350; } }` | IPC `NacreEdgeTriggerStream`: events `EdgeApproach`, `EdgeHover`, `EdgeTriggered`, `EdgeLeft` |
| **Overview hover**: the overview reports which workspace or window the pointer is on, so a shell can select without scrolling. | `nacre { overview-hover; }` | IPC `NacreOverviewHoverStream`; `niri msg window-under-cursor` |
| **Layer motion and promotion**: layer surfaces (bars, panels) animate open and close in the compositor and can be promoted to a floating window. | protocols `bead-layer-motion-v1`, `bead-layer-promote-v1` | used by Bead shells |
| **Proportional X11 widths**: tiled X11 windows stop forcing their minimum width. | `nacre { proportional-x11-widths; }` | — |
| **Window restore and force close** | `nacre { window-restore; force-close; }` | `restore-last-hidden-window`, `restore-current-workspace-hidden-windows`, `force-close-window` |
| **Monitor workspace actions**: swap or rotate workspace sets between monitors. | `nacre { monitor-workspace-actions; }` | `swap-workspace-with-monitor-next/previous`, `rotate-workspace-sets-next/previous` |
| **Launch workspace affinity**: an app launched with a token opens where it was launched from. | — | IPC `NacrePrepareLaunch` → `NacreLaunchToken` |
| **Lock readiness and fail-closed lock** | — | `hyprland-lock-notify-v1` lets hypridle delay suspend until every output is locked and drawn; lock acquisition never leaves the session uncovered |
| **Shake to enlarge the cursor** | `cursor { shake-to-enlarge; }` | — |

`niri msg nacre-capabilities` prints which of these are active. The full
`nacre { }` block: `overview-hover`, `scratchpad`, `window-pinning`,
`proportional-x11-widths`, `edge-triggers`, `window-restore`, `force-close`,
`monitor-workspace-actions`. Each is on by default; `nacre { edge-triggers false; }`
turns one off.

Default binds the fork ships on top of upstream: `Mod+Tab` toggle overview,
`Mod+Ctrl+Tab` / `Mod+Ctrl+Shift+Tab` next and previous collection, `Mod+Ctrl+N`
new collection, `Mod+Ctrl+Shift+Page_Down` / `Page_Up` move the window between
collections, `Mod+M` maximize column.

The map of every change is [docs/nacre/differences-from-niri.md](docs/nacre/differences-from-niri.md);
each feature's contract is under [docs/nacre/](docs/nacre/README.md). Everything
else behaves as the [niri wiki](https://niri-wm.github.io/niri/) documents
(a copy is in `docs/wiki/`).

## Install

**Arch and CachyOS, with Nacre:** the Nacre installer adds the signed repository
that carries `nacre-niri`; choose the nacre-niri session in `nacre setup`.

**Arch, standalone:**

```sh
cd packaging/arch
makepkg -si
```

The package depends on stock `niri>=26.04` and keeps it installed for rollback.
It adds the `nacre-niri-session` wrapper, its own user units and the
`nacre-niri-fork.desktop` session entry; stock `niri.desktop` is untouched.

**From source:** build prerequisites are upstream's
([development setup](https://github.com/niri-wm/niri/blob/main/CONTRIBUTING.md#development-setup)).

```sh
cargo build --release
./target/release/niri --help
./target/release/niri validate --config resources/default-config.kdl
```

The binary is still called `niri`; the Arch package installs it as
`/usr/bin/nacre-niri`.

## Run

Standalone, the session wrapper reads `~/.config/nacre/niri/config.kdl` (or
Nacre's prepared session when one exists) and, if present, a `nacre-niri.kdl`
overlay beside it for fork-only actions. Start with a copy of
`resources/default-config.kdl`; it already contains the collection binds and the
layer rules Nacre's wallpaper and bar use.

## How the family fits together

```text
nacre-niri   compositor (niri fork): windows, workspaces, collections, lock
   ├── nacre        the desktop: bar, launcher, panels, settings, daemon, installer
   │     ├── bead   the shell framework the UI is written with (Rust + Luau, MIT)
   │     └── orient the colour engine that turns a wallpaper into a palette (MIT)
   └── clasp        the login and lock screen (greetd), also built on Bead
```

## Upstream and contributing

This project is a fork of [niri-wm/niri](https://github.com/niri-wm/niri),
created by Ivan Molodetskikh. The upstream project and its contributors retain
their attribution and credit; consult upstream niri for general project
information and documentation. The fork branches from upstream `v26.04`
(merge-base 8ed0da44) and merges upstream selectively; the `nacre` branch is the
default. General compositor bugs belong upstream. Fork-specific issues and
changes go here; run `cargo test --all --exclude niri-visual-tests` and
`cargo run -- validate --config resources/default-config.kdl` first (the
[CI workflow](.github/workflows/ci.yml) lists the Ubuntu packages).

## Repository layout

- `src/` — compositor implementation
- `niri-config/`, `niri-ipc/` — configuration and IPC crates
- `resources/` — protocol definitions and default configuration
- `packaging/` — distribution and session packaging
- `docs/wiki/` — upstream niri documentation retained with this fork
- `docs/nacre/` — fork-specific behavior and compatibility contracts

## License

GPL-3.0-or-later, in the same license family as upstream; see [`LICENSE`](LICENSE).
The vendored Bead protocol XML (MIT) and hyprland-protocols (BSD-3-Clause) keep
their notices in [`docs/nacre/licenses/`](docs/nacre/licenses/).
