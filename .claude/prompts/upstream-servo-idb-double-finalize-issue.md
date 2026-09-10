## Summary

`IDBDatabase::clear_upgrade_transaction` panics ("clear_upgrade_transaction called but no upgrade transaction is set") when a version-change transaction's commit result and abort acknowledgment race: both the complete-notification and abort-notification tasks get queued, and neither task body re-checks the transaction's `finished` flag before clearing the upgrade transaction.

Observed on a vendored snapshot of components/script (2026-08-13 HEAD basis); the relevant code walk below uses function names from that snapshot.

## Steps to reproduce

Deterministic against real-world pages that probe IndexedDB from page JS (observed 100% on meituan.com's WAF script: open DB with version → create stores in `upgradeneeded` → abort racing the auto-commit). Minimal shape:

```js
const r = indexedDB.open('x', 1);
r.onupgradeneeded = () => {
  r.transaction.createObjectStore('s');
  // abort racing the auto-commit of the now-quiescent upgrade transaction
};
```

## What happens

Lifecycle walk (`dom/indexeddb/idbtransaction.rs`):

- `finalize_commit()` and `finalize_abort()` guard double-finalization with `if self.finished.get() { return; }` — but that guard only runs at **enqueue** time, and `finished` is set **inside** the queued task body.
- When the backend commit result (Ok) and the abort ack both arrive, `dispatch_complete()` and `finalize_abort()` each queue their notification task while `finished` is still `false`.
- Whichever body runs first performs `this.db.clear_upgrade_transaction(&this)` (clearing the connection's upgrade transaction) and sets `finished = true`.
- The second body runs unconditionally: for the abort task that means `clear_upgrade_transaction` again, whose `expect("clear_upgrade_transaction called but no upgrade transaction is set")` now panics — killing the whole ScriptThread.

Instrumented sequence from a live repro (one connection, one version-change tx, serial 5):

```
set_transaction            tx=T5
request_backend_abort      tx=T5   (x2 calls; second guarded)
dispatch_complete          tx=T5 finished=false   <- commit result Ok
finalize_abort             tx=T5 finished=false   <- abort ack
clear_upgrade_transaction  current=T5  (complete body first — ok)
clear_upgrade_transaction  current=0x0 (abort body — panic)
```

## Expected behavior

Per the IndexedDB spec's transaction-lifetime step ("when a transaction is committed or aborted, its state is set to finished"), a late finalization on an already-finished transaction should be a no-op, never a second lifecycle transition. Re-checking `finished` at the top of both task bodies (`send_complete_notification` / `send_abort_notification`) makes the first finalization win and the late one a no-op.

Happy to turn this into a PR if the maintainers agree with the analysis — the guard is two lines in each task body.
