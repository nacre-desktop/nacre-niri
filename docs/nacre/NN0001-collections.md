# NN0001: workspace collections

Collections give a workspace stack a stable identity independent of its output.
The fork enables them by default. `layout { collections { on false; } }` selects
upstream behavior, including appending unplugged workspaces onto the primary output.

## Ownership and compatibility boundary

`Layout` owns the collection registry: stable IDs, names, original-output identity,
persistence, hidden workspace stacks, and the saved active workspace.
Each `Monitor` references its shown collection ID. A shown collection leases its
workspace vector to the existing monitor presentation engine; hiding it returns
that vector to the collection. This keeps upstream scrolling, workspace animation,
and output geometry operations intact without unsafe pointers or shared mutable
workspace ownership. Collection transitions are centralized in `layout/collections.rs`.
The lease is an implementation accommodation for rebasing, not a second stack:
every workspace belongs to exactly one collection and exists in exactly one place.
Global workspace/window iterators include hidden collections. Hidden windows stay
mapped, receive commits/config updates, and are deactivated and leave their output.

Consumers of those iterators must handle an absent monitor even while outputs are
connected. The recent-windows switcher lists windows in shown collections only;
hidden windows become eligible after their collection is shown. Overview previews
may refer to hidden workspaces, whose collection-local index must never be used
as an index into a shown monitor's workspace stack.

## Output lifetime

An output initially receives its original collection, or a new collection.
Unplug returns its entire stack to the registry, preserving workspace IDs, order,
active workspace, and original-output identity. No workspace is appended elsewhere.
With no outputs, collection stacks remain independently stored.
On replug, the original collection returns to its output. If another output was
temporarily showing it, that output returns to its saved collection. When selecting
a collection already displayed elsewhere, swap the displayed collections so every
connected output continues to have a stack and no collection has two owners.
Workspace-level original-output behavior remains available to upstream explicit
workspace-to-output actions; collection restoration uses collection identity.

## Overview

Each output's overview contains its own shown collection and hidden collections,
including detached and virtual collections. A collection currently shown on another
connected output appears only on that output. The current collection stays centered
and uses the native monitor overview path, including its workspace animation and
hover camera. Hidden columns alternate right and left in the remaining side space;
they do not shrink the current collection. Each column has a small name/index label.
Workspaces remain vertical within their collection.

Hidden workspace windows, backgrounds, and shadows share one preview transform for
rendering and hit testing. Their positions are relative to the collection's saved
active workspace. Pointer selection resolves a workspace ID together with its collection;
activation first shows that collection on the clicked output, then focuses the
workspace through the existing overview close path. Dragging resolves the same
column/workspace target and uses the existing tile insertion machinery.

## Configuration, actions, and IPC

```kdl
layout {
    collections {
        on
        collection "work" {}
        collection "personal" {}
    }
}
binds {
    Mod+Ctrl+Right { focus-collection-next; }
    Mod+Ctrl+Left { focus-collection-previous; }
    Mod+Ctrl+1 { focus-collection "work"; }
    Mod+Ctrl+N { create-collection; }
    Mod+Ctrl+Shift+N { set-collection-name "work" "research"; }
    Mod+Ctrl+Shift+Right { move-window-to-collection-next; }
}
gestures {
    collection-switch { fingers 4; }
}
```

`create-collection` accepts an optional name and shows the new collection.
`focus-collection` and `move-window-to-collection` accept a one-based index or name;
IPC references additionally support stable IDs. Next/previous wrap in collection
order. Invalid references are harmless. Moving a window preserves its tile state.
The move actions target the collection's active workspace.
Config-declared names are reconciled on reload and remain while empty.
Legacy workspace move/index operations require a shown collection; hidden named/ID
references are rejected rather than interpreted on an unrelated monitor. Workspace
focus by name/ID first shows the owning collection.

`niri msg collections` / `Request::Collections` lists stable ID, one-based index,
name, displayed output (null when hidden), active/focused status, and workspace IDs.
`CollectionsChanged` replaces the collection state, follows normal layout refresh,
and participates in collection-aware event-stream state replication. Workspace and window
events continue to expose hidden windows; collection membership is obtained from
workspace IDs. Clients that need collection updates request the Nacre-Niri extension
with `niri msg event-stream --collections`; the default event stream omits collection
events so stock Niri 26.04 clients can still parse the stream.
`CollectionActivated { id }` identifies the newly shown collection. Clients that
also need overview hover metadata request `NacreOverviewHoverStreamWithCollections`;
the combined request is accepted only when both native features are enabled.
Showing a collection changes the standard workspace snapshot: newly shown
workspaces have their output and active/focused flags, while hidden workspaces
have no output and are inactive and unfocused. Stock clients do not need the
collection event extension to observe those changes.

## Empty removal

A shown collection is retained, even when empty. A hidden collection with no windows
on any workspace is removed, whether detached or user-created, unless declared in
config. A connected output's automatically created home desk remains available for
rotation while that output is connected. Runtime-created or renamed collections
with a non-empty name remain persistent while empty. Clearing the name makes an
otherwise empty hidden collection eligible for cleanup. Config-declared names
remain persistent and take precedence when configuration reloads. Empty trailing
workspaces continue to follow upstream workspace rules within each shown stack.
The named collection and workspace stack state file, routing rules, and restore
limits are documented in [collection persistence](collection-persistence.md).
Removal runs after window close/move and during refresh; invalid saved return IDs
fall back to an available collection.

## Validation and scope

Inherited hotplug assertions run with collections disabled. The enabled suite adds
unplug/replug identity, borrowed-collection restoration, empty removal, virtual
rotation, cross-collection moves, and shared-column hit tests. Operation-based
property tests interleave collection actions with output and window operations.
Builds/tests run only in the isolated dev-1 Arch container. Nested capture is only
allowed on the desktop test host, using private sockets/config and the capture
guard; installed niri and live configuration remain untouched.
