# NN0027: promoted panel drag input and cursor

Promoted layer panels remain above layout windows for pointer actions as well
as normal pointer delivery. The window activation lookup uses the presentation
hit test to exclude an obscured layout target, including during layer motion.
This preserves surface input regions, workspace visibility and promotion order.
Window activation regions outside a promoted panel keep their normal behavior.

The shell starts a native panel move with its existing Super+left-drag gesture
and the promotion protocol's validated pointer-button serial. The compositor
keeps the original surface/child pointer stream for movement and docking. It
shows the same Grabbing cursor as a window move and resets it when the grab
ends, including invalidation. Client cursor requests cannot replace that cursor
while a native layer move is active; screenshot mode retains cursor priority.

`src/tests/layer_promote.rs` covers initial interactive promotion, subsequent
Super+left-button input over a tiled window, client cursor override, movement,
release and stale serial rejection. The input regression uses the compositor's
virtual-pointer dispatch path so it exercises window selection before delivery.
