# Nacre Niri

Nacre Niri is Nacre's maintained fork of the [niri scrollable-tiling Wayland compositor](https://github.com/niri-wm/niri). It keeps niri's compositor architecture and adds native capabilities used by Nacre, including workspace collections and compositor protocols for Nacre/Bead surfaces. It is not a desktop shell; the Bead shell is maintained in the [Nacre repository](https://github.com/nacre-desktop/nacre).

This fork is actively developed for Nacre and selectively tracks upstream niri. See the [fork-specific documentation index](docs/nacre/README.md) for current behavior and compatibility notes. General compositor documentation remains in the [upstream niri documentation](https://niri-wm.github.io/niri/).

## What this fork adds

- Workspace collections with stable identities, independent stacks, and persistence.
- Native layer-surface motion and promotion, scratchpad pinning, proportional widths and edge cues.
- Native workspace overview geometry, GPU reveal, and input behavior for Nacre's shell integration.

These features are maintained in this fork; see the linked contracts for their current scope. This README does not imply complete parity with the latest upstream release.

## Build

Build prerequisites and system libraries follow upstream niri's [development setup](https://github.com/niri-wm/niri/blob/main/CONTRIBUTING.md#development-setup). With those installed, build the compositor from this checkout:

```sh
cargo build --release
```

The repository's [CI workflow](.github/workflows/ci.yml) documents the supported build and test commands and required Ubuntu packages.

## Run

To inspect the command-line options without starting a compositor session:

```sh
cargo run -- --help
```

Niri is a Wayland compositor, not a complete desktop environment. To run it as a session, use the session and packaging instructions for your system; the upstream [Getting Started](https://niri-wm.github.io/niri/Getting-Started.html) guide explains the compositor setup and safety considerations. The upstream [configuration guide](https://niri-wm.github.io/niri/Configuration%3A-Introduction.html) covers configuration shared with niri. Nacre-specific integration is documented under [`docs/nacre/`](docs/nacre/README.md).

## Repository layout

- `src/` — compositor implementation
- `niri-config/`, `niri-ipc/` — configuration and IPC crates
- `resources/` — protocol definitions and default configuration
- `packaging/` — distribution and session packaging
- `docs/wiki/` — upstream niri documentation retained with this fork
- `docs/nacre/` — fork-specific behavior and compatibility contracts

## Upstream and contributions

This project is a fork of [niri-wm/niri](https://github.com/niri-wm/niri), created by Ivan Molodetskikh. The upstream project and its contributors retain their attribution and credit; consult upstream [niri](https://github.com/niri-wm/niri) for general project information and documentation. This fork tracks upstream selectively to preserve its Nacre integration. Fork-specific behavior is documented under [`docs/nacre/`](docs/nacre/README.md).

## License

GPL-3.0-or-later, in the same license family as upstream; see [`LICENSE`](LICENSE). Vendored Bead protocol definitions retain their MIT notices in [`docs/nacre/licenses/`](docs/nacre/licenses/).
