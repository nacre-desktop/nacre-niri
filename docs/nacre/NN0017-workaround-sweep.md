# NN0017 / N0121: final workaround sweep

This audit starts at nacre-niri `835ed507` and Nacre `88cb0f2f`, after the
NN0015 integration. It supersedes the remaining-work dispositions in
[NN0002](NN0002-nacre-integration.md), rather than rewriting that historical
audit. NN0016 owns GPU reveal, layer/render changes and slides; those areas are
outside this sweep. Concurrent NN0018 desk overview/animation work is also
outside this sweep. No package, installation or live configuration is involved.

Decisions: **A** implement a small native feature now; **B** native behavior
already exists, so retire the corresponding Nacre workaround when available;
**C** retain Nacre product policy or ordinary backend integration; **D** defer a
real compositor gap. Sizes estimate implementation and meaningful regression
coverage: S = up to two developer days, M = roughly a week, L = several weeks.
They are estimates, not delivery commitments. A disabled capability always keeps
the stock fallback or the existing unsupported result.

## Runtime and configuration inventory

Nacre paths below are relative to the Nacre repository. Fork paths are marked
explicitly. A helper is removed only if neither compositor path uses it.

| Workaround / gap and source | State after NN0001–NN0015 | Decision and disposition |
| --- | --- | --- |
| Reconstruct workspace stacks and return windows after output reconnect (`compositor/niri_workspace_policy.rs`) | NN0001 collections retain stack identity and original-output restoration, including hidden and monitorless stacks. N0107 already skips legacy reconnect focus restoration with the active capability. | **B** Use native collections; keep the stock reconnect path. Do not infer readiness from the binary suffix. |
| Fixed workspace count, names, primary selection, primary-dynamic/fixed-all routing (`niri_workspace_policy.rs`) | Native collections do not choose a user's six numeric slots or which output owns them. | **C** These are saved Nacre preferences. Keep naming, profile migration, ownership persistence and bar presentation in Nacre. |
| Workspace visitor labels, occupied-only presentation, debounce and adoption ledger (`niri_workspace_policy.rs`) | Collection identity is native; labels and fixed-slot merge destinations are Nacre policy. | **C** Keep presentation and preference-driven destinations. **D/M** A generic bulk window transfer/adoption transaction would need a contract for pinned/scratchpad windows, hidden collections, rollback and origin changes; do not call the existing move-workspace action equivalent to visitor adoption. |
| Bring visitors here (`niri/workspace-visitors.kdl`, `niri_workspace_policy::bring`) | Existing helper adopts the selected visitor in dynamic mode, or merges visitor windows into matching local numbered slots in fixed-all mode. | **C** Destination selection remains policy. **D/M** Native atomic merge/adoption parity is deferred with the contract above; a plain workspace move does not satisfy it. Retain the helper on both paths until then. |
| Monitor-relative tile sends (`niri/monitor-workspace-actions.kdl`, `niri_workspaces.rs`) | Existing helper resolves configured destination slot and sends an implicit selected window, retaining overview/source focus. | **C** Resolve configured slot names in Nacre. Existing native `MoveWindowToWorkspace` does the actual move. No invented fork-only numbered-workspace convention. |
| Exchange selected workspaces across monitors (`niri_workspaces::assign`) | Helper uses multiple IPC moves/reorders/focus requests plus recovery and preference persistence. Individual upstream moves are not an atomic exchange. | **A** Native atomic monitor-relative exchange; **C** Nacre keeps fixed-slot admission and preference persistence. Capability-gate any equivalent fast path. |
| Rotate whole monitor workspace sets (`niri_workspaces::assign`) | NN0001 collections provide native whole-stack identities; helper still reconstructs named-set moves. | **A** Native workspace-set rotation in physical monitor order with physical focus retained. Collection identities remain attached to their physical outputs; moved workspaces explicitly adopt their destination output, preserving the assignment after reconnect. **C** Nacre fixed named-set membership and ownership persistence remain policy; use the native fast path only for an equivalent complete configured set with empty boundary rows. |
| Group toggle/cycle bindings (`compositor_config/niri.rs`, generated `niri/bindings.kdl`) | Both stock and fork already support tabbed columns and wrapping focus within a column; generated binds unnecessarily spawn a shell and `niri msg`. | **B** Emit direct `toggle-column-tabbed-display`, `focus-window-up-or-bottom`, `focus-window-down-or-top` on both paths. No new group model or capability is needed. |
| Close/fullscreen/maximize/float keybindings | Existing stock native actions already implement the corresponding generic operation. Backend helper API is still used by UI callers. | **B** Use direct KDL where the neutral action contract is exactly equivalent; retain backend methods for non-binding callers and policy-dependent operations. |
| Alt-tab `cycle_window` (`niri_actions.rs`) | Nacre cycles all windows in the active workspace by stable window ID; wrapping column focus is a different operation. | **C** This selection order is product policy. Keep it; do not replace it with group-cycle. |
| Hide/scratchpad and floating pin (`niri_actions.rs`, generated fork overlay) | NN0008 implements native scratchpad and pinning; N0114 emits fork-only actions and app pin rules. Nacre's reducer still hardcodes hidden/pinned false instead of consuming optional native metadata. | **B** Consume native window state and route native actions under active `scratchpad` / `window_pinning`; missing fields keep stock false. A stashed window is not an interactive move. Stock and disabled flags retain unsupported behavior; do not emulate pinning with repeated moves. |
| Restore last / restore current workspace (Super+Shift+H / Super+Alt+H) | N0114 maps restore-last to scratchpad cycling, an approximation that does not return the window to its original workspace; restore-current remains unsupported. | **A** Origin-aware native restoration. Restore-last returns newest hidden window to its source and focuses it; restore-current restores all hidden windows originating on the selected workspace without stealing focus. Preserve tiled/floating state. If an unnamed source workspace has expired, restore-last falls back to the active workspace. Independent `window_restore` capability requires scratchpad enabled. |
| Force-close a stuck window (Super+Ctrl+Shift+Q, outside the original 32 records) | `CloseWindow` only requests cooperative close; scratchpad hide is not force-close. The old audit conflated this with Super+Alt+H. | **A** Native client force-close behind an independent flag/capability. Record the client-connection scope and managed-X11 limitation; never disconnect the shared satellite to target one X11 app. |
| Pseudo-tile and split-toggle equivalence | Columns, tabs and floating are available, but none preserves Hyprland pseudo/split semantics. | **D/M–L** Needs agreed niri behavior, sizing/constraint handling, interaction and property tests. Avoid adding a misleading alias. |
| X11 widths service, standalone runtime and daemon observer (`niri_widths/*`, `daemon/widths.rs`) | NN0010 handles managed-satellite proportional tiled widths; N0115 gates startup and daemon observation with `proportional_x11_widths`. | **B** Skip X11 observation only while active capability is true; retain event-driven stock/external-satellite fallback. Helpers remain used and cannot be deleted. |
| Exact X11 type/constraint classification | Satellite does not forward `_NET_WM_WINDOW_TYPE`; unparented non-normal toplevels can receive normal width treatment. External satellites lack native provenance. | **D/M** Requires satellite metadata/protocol work and real X11 parity proof. The capability is not a claim to classify arbitrary external Xwayland clients. See [NN0010](NN0010-native-widths-edges.md). |
| Private patched overview-hover companion (`niri_session_runtime.rs`, `tools/build-niri-overview-hover.py`, patches) | NN0002 provides native hover metadata/selection. N0107 selects the fork directly and does not substitute a companion when hover is explicitly disabled. | **B** Skip companion on the fork. **C** Keep stock companion support and developer probes while stock still uses them. |
| Invisible shell edge surfaces / native edge stream | NN0010 emits edge trigger/leave events. N0115 switches source only after a successful stream acknowledgement and restores fallback on disconnect. | **B** Keep native takeover and reload handling; stock retains input surfaces. **C** Panel choice, dwell settings and lip hit testing remain product/UI policy. |
| Niri event adapters and snapshot reducers (`compositor/niri_events.rs`, `niri_state.rs`, `daemon/compositor.rs`, `daemon/niri_events.rs`) | Persistent event streams already carry workspace/window/focus/overview changes. Outputs are queried initially and after WorkspacesChanged/ConfigLoaded, not on a timer. | **C** These adapt IPC to Nacre's daemon contract; they are not missing-compositor-feature emulation. No periodic poller remains to replace. |
| Capability reload checks | The daemon re-queries capabilities on ConfigLoaded; width and edge owners react to the updated state. | **B** Use existing reload invalidation. No second periodic capability poll or redundant event is needed. |
| Output metadata invalidation | No dedicated output snapshot event; Nacre refreshes after workspace/config topology invalidation. | **D/S–M** A negotiated output-state stream could reduce event-triggered queries and cover independently changed output metadata. It does not remove a periodic poller, and requires compatibility, ordering and hotplug tests. |
| One-shot `power-baseline.sh`, `software_hardware.rs`, diagnostics | Queries run on explicit diagnostic or hardware-information requests. | **C** On-demand snapshots are normal IPC usage. They are not a polling workaround. |
| Reconnect backoff, readiness waits and opening-width deadline | Sleeps/timers wait after disconnect or bound one operation; width `poll(2)` waits on Niri/X11/shutdown file descriptors. | **C** Keep bounded lifecycle/error handling. Replacing these with new compositor events would not remove polling because they are already event waits. |
| `niri_hover_probe.rs`, `niri_preview.rs`, acceptance monitor and verify tools | Proof, private preview and session acceptance/recovery tooling; test probes use bounded waits. | **C** These test Nacre integration, not end-user compositor behavior. Keep them and their fixtures. |
| Panel/Pearl redock artifacts and UI restoration | NN0001 output restoration concerns workspace collections, not panel UI state. | **C** Keep Nacre restoration and motion tests. NN0016 owns current visual reveal work. |
| Native detach, layer motion, overview geometry and desk previews | Reviewed NN0006/7/9/12–15 documents describe native support and integrated fixes. | **B** Keep using native protocols. **C** Client rendering, shell presentation and acceptance tooling remain outside the compositor. GPU reveal/visual acceptance stays with NN0016; this sweep does not claim that work complete. |
| Generated include graph, startup, host output/keyboard settings, palettes, app rules and custom include | NN0002 moved portable defaults to the fork, but explicit configs do not implicitly inherit bundled defaults. | **C** Nacre owns session assembly, theme and app policy. Keep shared fragments; put every fork-only addition in `config/nacre-niri.kdl`, including native pin and edge config. User overrides retain their established order. |
| Night light, display transactions and workspace preference migration (`niri_night_light.rs`, `niri_desktop.rs`, `niri_config.rs`) | Normal compositor/backend integration plus Nacre-owned settings and wlsunset lifecycle. | **C** Keep these adapters; no evidence that an IPC event can replace the external color-controller or settings writer contract. |
| Cursor coordinates, shaped background blur, native panel drag, monitor mirroring, richer binding inspection | Capability gaps remain; no fake runtime implementation is removed by the shipped units. | **D/M–L each** Needs protocol/input/security/render design and compositor/client tests. Panel drag/non-consuming input require client-isolation semantics; defer rather than translate backend-specific hooks. |

## All 32 stock unsupported records

This table counts records, including repeated IDs in distinct source contexts.
Stock `niri/unsupported.json` stays truthful. Fork metadata qualifies satisfied
rows by compositor and active capability; disabling a flag must not advertise
that behavior. NN0008/N0114 address five records exactly (hide, pin binding and
three app pin rules); their sixth filtered record, restore-last, previously used
the approximate scratchpad-toggle mapping. NN0017 corrects that distinction.

| Count | Record IDs / source | After NN0015 and sweep decision |
| ---: | --- | --- |
| 1 | `SUPER + H`, keybinding.lua | **B** NN0008 move-to-scratchpad supplies hide when `scratchpad` is enabled; no claim of a separate trash history. |
| 1 | `SUPER + SHIFT + H`, keybinding.lua | **A** Native restore-last with `window_restore`; old scratchpad cycling is not exact parity. |
| 1 | `SUPER + ALT + H`, keybinding.lua | **A** Native restore-current-workspace with `window_restore`. Neutral source is `window-minimize.sh restore-current`, not force-kill. |
| 1 | `SUPER + P`, keybinding.lua | **B** NN0008 floating pinning; active `window_pinning` required. |
| 1 | `SUPER + SHIFT + P`, keybinding.lua | **D/M–L** Pseudo-tile semantics need design. |
| 1 | `SUPER + J`, keybinding.lua | **D/M–L** Split toggle has no exact scrolling-layout equivalent. |
| 1 | `SUPER + mouse:273`, keybinding.lua | **C** Keep native move/resize gestures; Nacre pointer-hook policy is not a generic replacement. |
| 1 | `SUPER + S`, keybinding.lua | **C/D/M** Neutral source requests a named Hyprland special workspace, not NN0008's per-window stash. Keep unsupported until that separate UX is specified. |
| 1 | `SUPER + CTRL + S`, keybinding.lua | **D/M** Named special workspace semantics are not collections or the per-window scratchpad. |
| 2 | `SUPER + R`, keybinding.lua (two contexts) | **D/M** Modal keymap/submap stack, entry/exit and input-isolation semantics. Do not confuse with the existing preset-width key. |
| 4 | `left`, `right`, `up`, `down`, keybinding.lua | **D/M** Resize actions exist, but these bindings belong to the unsupported modal resize map. Global arrow interception would change client input. |
| 2 | `escape`, `return`, keybinding.lua | **D/M** Exiting the unsupported resize keymap needs the same modal-input contract. |
| 4 | `nacre-thunar-files`, `nacre-dolphin-files`, `nacre-widgets-window` (windowrule.lua), `nacre-lock-preview` (nacre.lua) | **C** App geometry remains Nacre policy; working-area proportions intentionally differ from raw output proportions. Metadata records that translation difference. |
| 3 | `nacre-audio-mixer`, `nacre-share-picker`, `nacre-picture-in-picture`, nacre.lua | **B** NN0008 `open-pinned` rules in the fork overlay, active pinning required. Stock retains three records. |
| 1 | Empty ID, backend-config, nacre.lua | **C** Backend-specific config has no automatic niri emitter; translate only concrete product settings with exact equivalents. |
| 1 | `Escape`, shortcuts.lua | **D/M–L** Non-consuming shortcut hooks require input/client-isolation design. |
| 1 | `SUPER + mouse:272`, unbind, shortcuts.lua | **C** Backend-specific unbind is not a missing native layout operation. |
| 1 | `panel.drag`, extension, shortcuts.lua | **D/L** Compositor/client drag protocol; preserve native move semantics meanwhile. |
| 1 | `SUPER + mouse:272`, binding, shortcuts.lua | **D/L** Panel-drag start uses that missing protocol. |
| 1 | `mouse:272`, binding, shortcuts.lua | **D/L** Panel-drag finish uses that missing protocol. |
| 1 | `cursor.session`, extension, cursor.lua | **C/D/M** Backend cursor-session policy needs an explicit Wayland/Niri contract before porting. |
| 1 | Empty ID, event, cursor.lua | **C/D/M** Backend-specific cursor event; same contract, not a generic event-name substitution. |

## Contract and verification

New actions are `restore-last-hidden-window`,
`restore-current-workspace-hidden-windows`, `force-close-window` (optional IPC
window ID), `swap-workspace-with-monitor-next` / `previous`, and
`rotate-workspace-sets-next` / `previous`. The independent flags default on;
explicit opt-out is:

```kdl
nacre {
    window-restore false
    force-close false
    monitor-workspace-actions false
}
```

`NacreCapabilities.window_restore` additionally requires `scratchpad`;
`monitor_workspace_actions` additionally requires collections. Missing new IPC
fields deserialize false for old versions. Existing `ConfigLoaded` invalidates
active capabilities. Group actions are upstream actions, so need no new flag.

Force-close disconnects the selected native Wayland client, including sibling
windows using its connection. It sends no OS process signal. Managed satellite
windows are excluded because disconnecting that shared bridge would affect other
X11 apps. **D/M** Per-X11-window force termination needs a satellite/window-ID
contract; external satellites have no managed provenance marker and are outside
the guaranteed scope. No promoted-layer or rendering behavior is changed.

Native implementation was validated at `5101b511a7a8798f28e60ba748739e7d3ec56f76`.
The complete Linux CI replica passed on dev-1: formatting, normal tests, default
config validation, no-default/dbus/systemd/dinit/xdp/default feature builds,
Tracy, visual-test build, Clippy, Rust 1.85 MSRV, randomized build/run and release
build/version. Every stage exited **0**. The slow randomized run used
`RUN_SLOW_TESTS=1 PROPTEST_CASES=200000`; its main library suite passed 378 tests
with two ignored. Clippy emitted warnings under the upstream job's warning
policy; it was not a warning-free run.

The full property run initially exposed a real rotation/unconfigure boundary
bug: clearing persistence on an empty two-row workspace set during a transition
could leave adjacent empty unretained rows. The fix cancels only that empty
transition and uses the existing cleanup path. A deterministic regression and
the saved property seed are retained; the monitorless and empty-row invariants
were not weakened. The corrected full 200,000-case run passed.

Earlier dev-1 builds/tests used task-owned containers limited to 8 CPUs and
16 GiB, with `nice -n 10` inside each container and Cargo parallelism limited to
two. Exact commands, source/tree identities, logs and numeric exit codes are
retained under the task worktrees' ignored `.lead/` evidence directories.

Hosted [CI run 41795656460](https://github.com/nacre-desktop/nacre-niri/actions/runs/41795656460)
on `8dcff015e53ff60993ae9fad1fd57368b92f1df7` passed the test, slow/randomized
(200,000 cases), feature combinations, MSRV, rustfmt, Clippy, visual build,
Fedora, Alpine and Nix jobs. Its overall result is failure solely because the
FreeBSD VM package repository requires a newer OS version than its userland;
that setup fails before compilation and is the owner-approved known exception.
The code is unchanged from the native revision above; subsequent changes are
documentation only.

Nacre full desktop validation remains deferred under the owner's revised
validation plan. Earlier setup and Clippy checks succeeded at
`ebb640f3c3fc0b0cbcd96d21703797d6072b825c`; a host reboot interrupted the
combined helper stage, so no full Nacre CI pass is claimed. The earlier
`2c9b45d78` run exited 101 for a new test fixture's JSON-map type; `ebb640f3c`
fixes it. Remaining desktop gates are the complete Nacre desktop workflow,
including workspace tests, stock/fork config validation with the freshly built
fork binary, helper parity, shell proofs, source/UI checks, galleries and daemon
memory checks. Repeat the native 200,000-case release suite there for the paired
handoff. Task `.lead/report.md` records exact desktop commands. No further dev-1
or production access is part of this unit's current validation plan.
