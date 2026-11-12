# NN0030 — session-lock owner checks from upstream Smithay

nacre-niri was pinned to Smithay `ff5fa7df`, the same revision as niri v26.04.
In that revision, `ext_session_lock_v1.unlock_and_destroy` on a lock object that
never received `locked` posts `InvalidUnlock` but still calls
`SessionLockHandler::unlock()`. A client whose lock request was refused while
another client held the lock could therefore end that lock.

Upstream Smithay fixed this on 2026-08-11. nacre-niri now builds against
[`nacre-desktop/smithay`](https://github.com/nacre-desktop/smithay/tree/nacre/ff5fa7df-session-lock-fix)
at `088a4795`, which is `ff5fa7df` plus these upstream commits, cherry-picked
with `-x` and adapted only to the pre-`Dispatch2` trait shapes:

- [8d33fec901](https://github.com/Smithay/smithay/commit/8d33fec9015820097185ebf7d9cd2029f61933f6):
  only the lock instance that locked the session can unlock or destroy it.
- [70f04625a9](https://github.com/Smithay/smithay/commit/70f04625a9d17813192541e332caf6f2bd72f275):
  refused lockers' surfaces no longer reach the compositor handler.
- [fea33c8f16](https://github.com/Smithay/smithay/commit/fea33c8f163a8bf820f1ce97947fbff44747e55d):
  a disconnected owner leaves the session defunct (still locked); takeover
  stays compositor policy (see [NN0029](NN0029-lock-fail-closed.md)).

niri matches lock surfaces to the lock instance (`LockSurface::ext_session_lock`)
instead of the client. The NN0029 guard that ignores `unlock()` before `Locked`
stays as defence in depth.

## Verification

`tests::session_lock::refused_client_cannot_unlock_another_clients_session`:
client A holds the lock, client B's request is refused, B sends
`unlock_and_destroy`; the session must stay locked by A, and A can still unlock.
It fails on `20907e11` with the old pin (11 passed, 1 failed) and passes with the
backport (12 passed). `cargo test --lib` (447 passed), Clippy and rustfmt pass;
Smithay's own check and session-lock doc tests pass on the branch.

## Removal

Drop the fork pin when nacre-niri moves to a niri release whose Smithay includes
these commits (niri `main` already does).
