# Named collection persistence

The Nacre-Niri fork stores named desk stacks and best-effort window placement rules at
`$XDG_STATE_HOME/nacre-niri/collections.json`. When `XDG_STATE_HOME` is unset it uses
`$HOME/.local/state/nacre-niri/collections.json`. This is runtime state, separate from the
generated KDL configuration. It never launches applications.

The versioned JSON records collection order and names, each collection's original output,
workspace order and optional names, and the active workspace ordinal. Runtime collection and
workspace IDs are not reused after restart. Named workspaces are recreated by name; unnamed
workspaces are recreated by their position in the saved stack. Empty-slot retention is stored per
workspace so compositor-generated trailing placeholders do not accumulate across restarts, while
empty slots targeted by remembered window rules remain available. Config-declared collection names
are reconciled first in config order through a separate configured-origin alias. If a configured
`work` desk is renamed to `research`, the saved `research` name remains visible while a later
`work` declaration attaches to that same stack without creating a duplicate. Persisted user-named
collections that are absent from config follow them in saved order. Explicit window rules such as
`open-on-workspace` and `open-on-output` take precedence over remembered window placement.

Window rules require an app-id, optionally include an exact title, and point to a named collection
plus either a workspace name or collection-local ordinal. Windows without an app-id cannot get a
remembered route. An exact app-id/title match wins. If the title changes,
an app-id fallback is used only when every saved route for that app-id names the same destination.
When two live windows share the same app-id/title but occupy different destinations, that key is
ambiguous and is omitted. There is no fuzzy title or regular-expression matching. Applications
with dynamic titles may therefore open on the active workspace when their title no longer matches;
this is best effort, not session restoration. Closed-window routes remain stored so their previous
placement can be used on a later launch.

The reader accepts only the current schema and rejects files over 4 MiB, more than 128 named
collections, more than 256 workspaces in one collection, or more than 4096 window routes.
Collection, configured-origin, output, and workspace names are limited to 256 UTF-8 bytes; app-ids
to 1024 bytes; and titles to 2048 bytes. Live rules with oversized app-ids or titles are omitted
without truncation. A stack with an oversized name or workspace list is omitted from the saved
snapshot, along with routes to it, while supported desks remain saveable. If the aggregate snapshot
would exceed 4 MiB, supported stacks and routes are kept in collection order until the file limit
is reached. These bounds affect restart persistence only; the live layout remains unchanged.
An unreadable, malformed or newer-version file is left untouched and persistence is skipped rather
than replacing state this version cannot interpret.

Writes use a same-directory temporary file with mode `0600`, sync its contents, atomically rename
it over the state file, then sync the directory. The state directory is mode `0700`. The compositor
debounces logical layout changes and writes only when the serialized state changes; it does not
write once per rendered frame. If state is missing, the compositor starts from config and normal
empty collection behavior.

When collections are disabled, persistence I/O is paused and the state file is left unchanged. If
collections are enabled later in the same compositor process, persistence resumes after restart;
the compositor does not load an old stack over windows already placed in the live layout.
