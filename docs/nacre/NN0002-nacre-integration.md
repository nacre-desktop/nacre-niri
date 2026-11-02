# NN0002: Nacre niri integration audit

This audit compares the generated Nacre niri session, Nacre's niri source files,
the private custom include, and the small helpers used to cover niri gaps. The
generated files are split by responsibility; machine output/keyboard settings,
Nacre product policy, and user overrides are not fork defaults. No private path,
machine identifier, palette value, or command line from the owner's custom file
is copied here.

Classification: **BUILT-IN DEFAULT** is portable behavior shipped in
`resources/default-config.kdl`; **NATIVE** is a small fork capability for a niri
limitation, guarded by a config flag that defaults on and preserves upstream
behavior when disabled; **STAYS IN NACRE** is product policy, host/theme data,
or bindings to Nacre commands.

| Touchpoint | Decision | Reason / disposition |
| --- | --- | --- |
| Generated `config.kdl`: include graph, session startup programs, hotkey overlay, and include ordering | STAYS IN NACRE | Nacre owns session lifecycle and generated-file assembly. Keep the user's `custom.kdl` included last through the existing Nacre mechanism, so overrides still win (N0103). A standalone fork default must not include a possibly missing owner/Nacre path. |
| Generated `desktop.kdl`: animation choice, effective 6-pixel gap / 1-pixel border, 10-pixel window corners, wallpaper blur namespaces, touchpad tuning, pointer focus, and the named laptop output/mode/scale/position | BUILT-IN DEFAULT for portable geometry, pointer focus, touchpad tuning, and wallpaper-layer blur; STAYS IN NACRE for output | Desktop settings override the host fragment's 12-pixel gap; the effective portable values are 6-pixel gaps, 1-pixel configured border and focus-ring widths, 10-pixel rounded window geometry with Pearl exclusions, and touchpad natural-scroll at 0.75. Nacre continues to own output identity/mode/scale/position. |
| Generated `host.kdl`: no-CSD, 12-pixel gap (overridden to 6), half-width default, 1/3–1/2–2/3–full presets, thin focus ring, touchpad tuning, named Nacre app width, and host bindings | BUILT-IN DEFAULT for no-CSD/layout/touchpad behavior; STAYS IN NACRE for app-specific rules and Nacre command bindings | Ship the effective desktop values in the fork. Portable direct niri actions `Super+Ctrl+Tab` (overview), `Super+M` (maximize-column), `Super+R` (preset width; already represented by `Mod+R` on TTY), and Shift+wheel focus belong in the fork default. |
| Generated `wallpaper.kdl` and `niri/wallpaper.kdl`: transparent background, wallpaper layer placement behind overview, overview workspace shadow | BUILT-IN DEFAULT | This is reusable compositor presentation behavior for Nacre/Bead wallpaper surfaces and needs no Nacre command or private path. The fork default includes it. |
| Generated `window-rules.kdl`, `niri/window-rules.kdl`, and compositor-contract output: generic rounded geometry, shadow/blur, Thunar/Dolphin/widgets/lock preview geometry, Nacre app floating rules, generic portal/control windows, PiP, and Pearl chrome | BUILT-IN DEFAULT for effective generic desktop rules and Pearl protocol; STAYS IN NACRE for product/app-specific rules | Ship the effective 10-pixel clipped corners (excluding Pearl), general shadow and background blur, and blur for the generated Nacre layer namespaces. Pearl rules later disable shadow/blur and decoration effects. Nacre keeps its file-manager, control, lock-preview, and other app-specific geometry. |
| Generated `bindings.kdl` and `niri/bindings.kdl`: workspace/window navigation plus Nacre launcher, palette, clipboard, widgets, power, lock, files, terminal, AI, settings, screenshot, and helper actions | BUILT-IN DEFAULT for portable direct niri actions; STAYS IN NACRE for Nacre commands and owner overrides | The fork defaults include overview toggle on `Super+Ctrl+Tab`, maximize-column on `Super+M`, preset width on the existing TTY `Mod+R` action, and the existing Shift+wheel column focus with the owner's cooldown. Binds to Nacre commands and the user's own `Super+Ctrl+Q`, `Super+Tab`, and screenshot/preview overrides remain in Nacre and external `custom.kdl`. |
| Generated `colors.kdl` and owner's palette include | STAYS IN NACRE | Palette colors and preserving owner-selected ring/border state are theme policy. The fork does not copy palette values. |
| Generated `keyboard.kdl` | STAYS IN NACRE | XKB layout/model/options are locale and device policy. |
| NN0001 collection registry and Nacre generated `workspaces.kdl` / `niri_workspace_policy.rs` | NATIVE (NN0001) for collection storage and output restoration; STAYS IN NACRE for named workspace policy | NN0001 owns stable collections, hidden stacks, original-output identity, and restoring the original collection when a monitor returns. It does not choose Nacre's fixed numeric slots/names or decide per-profile routing. The helper may continue applying that Nacre policy over the native collection API. |
| Generated `monitor-workspace-actions.kdl`, `workspace-visitors.kdl`, and `niri/monitor-workspace-actions.kdl`, `niri/workspace-visitors.kdl` | STAYS IN NACRE | Nacre's helper implements monitor send/swap/rotate and visitor adoption/bring policy using niri IPC. NN0001 supplies generic collection primitives and origin identity; it does not supply those Nacre policy operations or bindings. |
| `nacre-niri-widths.service`, `niri_widths.rs`, width CLI/runtime/X11 modules | NATIVE, deferred to a later NN width-policy unit | The helper relaxes horizontal X11 minimum-size hints only for ordinary tiled windows, preserves fixed/max/height/increment/aspect hints, and restores constraints when floating or stopped. A default width alone is not equivalent. Keep the helper active until native parity is demonstrated. |
| `niri_hover_probe.rs`, hover pointer support, patched overview-hover binary/source under the private companion checkout, session runtime verification | NATIVE (NN0002) | The default-on `nacre.overview-hover` flag enables opt-in hover metadata and zero-scroll pointer selection. Hidden collection previews report metadata without changing keyboard focus. Nacre selects the fork instead of its patched companion; the proof probe stays developer tooling. Explicit flag-off uses upstream behavior and a rejected hover-stream handshake falls back to standard events. |
| `niri-redock-*` state artifacts and Nacre panel/Pearl redock behavior | STAYS IN NACRE | Inspection of the artifacts and `test_panel_redock_motion.py` shows this is Nacre UI restoration/motion testing, not a runtime compositor helper that remembers window outputs. NN0001 collection-origin restoration is a separate capability for workspace stacks after output unplug/replug; it does not replace panel redock animation or restore. Keep the UI behavior and its tests in Nacre; no later native unit is proposed. |
| `niri_acceptance_monitor.rs`, acceptance systemd watcher, and acceptance tests | STAYS IN NACRE / test harness | This monitors Nacre's generated runtime acceptance conditions and session recovery. It is not end-user compositor behavior and should not run as a fork feature. Keep it in Nacre CI/acceptance tooling. |
| `bead-pearls.kdl` and Nacre's embedded Pearl fragment | BUILT-IN DEFAULT | Pearl bootstrap/floating/opacity and chrome exclusions implement the Pearl window protocol, not a theme or Nacre command. Retain the N0103 include order so custom overrides apply after managed fragments. |
| `unsupported.json` (32 records) | STAYS IN NACRE except any item explicitly replaced by a shipped native feature | It is an audit of Nacre's Hyprland-to-niri translation, not a compositor config. The 20 missing/consuming bindings, 7 app window rules, 2 extensions, backend config, unbind, and cursor event remain Nacre capability metadata. Existing reasons identify unavailable hide/trash, pin/pseudo, split toggle, scratchpad/keymap/resize, panel drag, non-consuming Escape, and backend-specific cursor behavior. Nacre may remove a record only when the corresponding behavior is actually implemented and covered. |

## Built-in defaults in this unit

The fork default carries the portable owner setup, with values taken from the
generated host and desktop fragments: `prefer-no-csd`, effective 6 logical pixel
gaps, 1-pixel configured border and focus-ring widths, a 50% default column, and
1/3, 1/2, 2/3, and full-width presets. The rules add 10-pixel clipped corners
outside Pearl windows, touchpad natural scrolling at 0.75, focus following only
within the visible area, a transparent background, wallpaper placement behind the
overview, layer blur, and general window shadow/blur, plus a soft overview shadow. It also carries the portable direct niri bindings and
Bead Pearl window contract; Pearl rules disable generic shadow and blur.
It does not set an output name/mode, keyboard
layout, palette, Nacre command bind, Nacre startup program, or a path to the
owner's custom file. Nacre keeps generating/includes its custom file after its
managed fragments, matching the existing custom-override contract.

Nacre can identify the fork from its version suffix or query its IPC capabilities.
Use fork identity only to select the compatibility path; skip each helper only
after IPC reports that the specific native feature is enabled. A disabled hover
flag deliberately selects upstream behavior, so Nacre does not substitute its
patched companion to turn hover back on. Stock niri keeps its existing companion
path. Native flags default on in the fork and leave upstream behavior intact when
explicitly disabled. The native items and current implementation status are tracked below; a helper
is not removed merely because a similar-looking rule exists.

| Candidate | NN disposition | Rationale / follow-up |
| --- | --- | --- |
| NN0001 collection registry and output restoration | Landed/base | Fork owns stable collection identities, hidden stacks and restoration to an original output after hotplug. Nacre retains fixed workspace naming/routing, visitor policy and monitor actions. |
| Proportional widths for X11/Xwayland clients | Later NN width-policy unit (unassigned) | Requires reliable Xwayland identification and tiled-only constraint handling with fixed-size/floating/max-hint parity. A placement rule cannot replace this contract. NN0003 is separately reserved for packaging. |
| Overview hover events/behavior | NN0002 | Ported against NN0001, including camera preservation, opt-in events and reload/off-state cleanup. |
| Redock of Nacre Pearl/panel UI | STAYS IN NACRE | The state artifacts are test/experiment evidence; the runtime behavior is Nacre panel restoration and motion, unrelated to compositor workspace collection output identity. |
| Remaining `unsupported.json` items | Later N-units only when a small native implementation has an end-user value | Most are absent niri concepts or Nacre command mappings. They remain Nacre-owned metadata until separately designed and tested. |

The `unsupported.json` inventory (32 records), exhaustively grouped:

| Count | IDs / source touchpoints | Current disposition |
| ---: | --- | --- |
| 3 | `SUPER + H`, `SUPER + SHIFT + H`, `SUPER + ALT + H` (hide/trash/force-kill) | STAYS IN NACRE metadata; no equivalent compositor operation. |
| 1 | `SUPER + P` (pin) | STAYS IN NACRE metadata; no native pin action. |
| 1 | `SUPER + SHIFT + P` (pseudo) | STAYS IN NACRE metadata; no native pseudo layout. |
| 1 | `SUPER + J` (toggle split) | STAYS IN NACRE metadata; no equivalent niri action. |
| 1 | `SUPER + mouse:273` | STAYS IN NACRE metadata; niri's native move/resize gestures remain in control. |
| 1 | `SUPER + S` (scratchpad window) | STAYS IN NACRE metadata; no equivalent scratchpad. |
| 1 | `SUPER + CTRL + S` (special workspace) | STAYS IN NACRE metadata; niri has no special workspace concept. |
| 2 | `SUPER + R` (keymap switch; repeated in two source contexts) | STAYS IN NACRE metadata; no keymap-switch capability. |
| 4 | `left`, `right`, `up`, `down` (resize action) | STAYS IN NACRE metadata; no equivalent action contract. |
| 2 | `escape`, `return` (keymap switch) | STAYS IN NACRE metadata; no keymap-switch capability. |
| 4 | `nacre-thunar-files`, `nacre-dolphin-files`, `nacre-widgets-window`, `nacre-lock-preview` | STAYS IN NACRE; unsupported size effects use compositor working-area proportions. |
| 3 | `nacre-audio-mixer`, `nacre-share-picker`, `nacre-picture-in-picture` | STAYS IN NACRE; pinned-window effect is unavailable. |
| 1 | `hypr/conf/nacre.lua` backend config | STAYS IN NACRE; backend-specific config has no niri emitter. |
| 1 | `Escape` non-consuming binding from `nacre/shell-tools/shortcuts.lua` | STAYS IN NACRE; niri consumes configured bindings. |
| 1 | `SUPER + mouse:272` unbind | STAYS IN NACRE; backend-specific unbind is not emitted. |
| 1 | `panel.drag` extension | STAYS IN NACRE; backend extension is not emitted. |
| 1 | `SUPER + mouse:272` (`panel.drag`) | STAYS IN NACRE; no equivalent drag action. |
| 1 | `mouse:272` (`panel.drag.finish`) | STAYS IN NACRE; no equivalent finish action. |
| 1 | `cursor.session` extension | STAYS IN NACRE; backend extension is not emitted. |
| 1 | `hypr/conf/cursor.lua` event | STAYS IN NACRE; backend-specific cursor event is not emitted. |

NN0002's hover flag defaults on in the configuration model. Explicit opt-out is
`nacre { overview-hover false; }`. Collections use the independent NN0001 flag
`layout { collections { on false; } }`.
The raw IPC request is the JSON string `"NacreCapabilities"`; the reply is
`{"Ok":{"NacreCapabilities":{"workspace_collections":true,"proportional_x11_widths":false,"overview_hover":true}}}`
with booleans reflecting the running configuration. The version suffix is
`(nacre-niri)`. N0107 uses binary identity before startup and active IPC flags
for runtime workarounds. It skips legacy reconnect focus restoration when
collections are enabled, retains fixed-slot/visitor product policy, and avoids
actions on hidden workspaces. The width helper remains active in this unit.

Other proposed NN follow-ups are intentionally unassigned: sticky floating
windows (pin semantics across workspace/collection changes), scratchpad/hide
actions (explicit collection lifetime and focus rules), and non-consuming input
hooks/submaps (input delivery and client isolation). None is a safe config-only
translation. Panel drag needs a compositor/client protocol design rather than a
binding substitution. Pseudo/split actions are Hyprland layout concepts; no
native port is proposed without an agreed niri behavior. App geometry and stale
translation metadata belong in later Nacre units. The inventory records each
current limitation rather than promising parity.

## Sources reviewed

Reviewed the generated session fragments (`config`, `desktop`, `host`,
`keyboard`, `colors`, `wallpaper`, `window-rules`, `bindings`, `workspaces`,
`workspace-visitors`, `monitor-workspace-actions`, `bead-pearls`, and
`unsupported`); Nacre `niri/` fragments and `unsupported.json`; the niri config
generator, compositor config/action/workspace modules, session runtime,
widths, hover probe, acceptance monitor and their service/test references; and
the owner's external `custom.kdl` only to verify the include/override contract.
Private values and commands from the custom file were excluded from this audit.
