//! Std-backend arena — replaces the mimalloc `mi_heap` arena backend
//! (user verdict 2026-09-20: remove all custom allocators; E22 terminal design
//! §1.2/§1.3 as refined by the 2026-09-20 E22 ruling on this slice). Stage-1
//! slice: new backend in a new file; the `lib.rs` wiring swap
//! (`pub type Arena = StdArena`, `pub use` re-exports) lands in the closure
//! segment, so this file touches nothing else.
//!
//! # Why an arena at all (consumer contract — four preserved semantics)
//!
//! Consumers (bundler / installer / js_printer) pick this over a bump arena
//! for exactly one reason: **every allocation is individually freeable**, so a
//! parse can release temporaries mid-flight and keep RSS bounded. The backend
//! swap preserves all four load-bearing semantics of `MimallocArena`:
//!
//! 1. **Per-allocation free** — `deallocate` unlinks the block O(1) through
//!    its own header and returns the whole block (header included) to libc
//!    *immediately*. This is what keeps RSS flat during a parse.
//! 2. **Bulk-free on `reset`/`Drop`, including leaked blocks** — the live
//!    chain *is* the live set: everything still chained was never
//!    deallocated, so the `reset`/`Drop` walk frees exactly those blocks and
//!    touches nothing that was already freed. There is no dead-block skip
//!    because dead blocks leave the chain at `deallocate` and cease to exist.
//! 3. **`Send + Sync`** — the arena can be moved across threads;
//!    `deallocate` may run on any thread and through any arena handle,
//!    provided the arena is *quiescent* — no `allocate`/`reset` running
//!    concurrently (those stay pinned to the constructing thread; see the
//!    `Send`/`Sync` SAFETY notes and the thread-contract note below).
//! 4. **Cross-arena free (`transfer_arena`)** — a block allocated by arena A
//!    may be deallocated through arena B's handle. With per-heap mimalloc
//!    this leaned on `mi_free`'s page-metadata lookup for the owning heap;
//!    here the ownership link *is the block header*: `next`/`pprev`
//!    self-describe the block's position in its owner's chain, so any handle
//!    from any thread can unlink and free it. `grow` matches the contract
//!    line for line as well: allocate the new block on `dst`, `memcpy` the
//!    `min(old, new)` prefix, then unlink-and-free the old block through its
//!    own header — the shape of "allocs on dst, memcpy, then mi_free(ptr)".
//!
//! # Block layout (E22 refined ruling)
//!
//! ```text
//! Block header, 24 B — the block's own permanent record, freed WITH it:
//!   ┌────────────────────┬────────────────────────┬──────────────────────┐
//!   │ next: *mut Block   │ pprev: *mut *mut Block │ payload: *mut u8     │
//!   │ chain link         │ addr of the predecessor│ null = single alloc  │
//!   │                    │ `next` slot (O(1)      │ (user = base + 24)   │
//!   │                    │ unlink, no walk, no    │ non-null = dual alloc│
//!   │                    │ prev node needed)      │ (= aligned payload's │
//!   │                    │                        │ malloc base)         │
//!   └────────────────────┴────────────────────────┴──────────────────────┘
//! single alloc (requested align ≤ 8 — the hot path):
//!   one plain malloc(24 + size): [Block 24 B][user bytes]; user = base + 24,
//!   which is 8-aligned and therefore satisfies every align ≤ 8.
//! dual alloc (requested align > 8):
//!   [plain malloc(24) header] + [plain malloc(8 + align + size) payload
//!   storage]; user = align_up(storage + 8, align); the storage word at
//!   user - 8 holds the storage's malloc base so `deallocate(user)` can find
//!   (and free) both allocations. Manual alignment inside a plain malloc
//!   block keeps every free in this module a plain `free` — no
//!   `_aligned_malloc`/`_aligned_free` pairing to track.
//! ```
//!
//! **The mode discriminator is the word at `user - 8`.** For a single alloc
//! that word *is* the header's `payload` field (base + 16), which is null;
//! for a dual alloc it is the storage back-link, which is never null. One
//! read decides the mode, recovers the base, and needs no size or alignment
//! in the header — `libc::free` is size- and alignment-agnostic, so the
//! header stays exactly the three pointers of the ruling.
//!
//! The chain threads through headers that die with their blocks, and
//! `deallocate` unlinks *before* freeing — which is what makes the design
//! sound where a "mark dead, free, and let `reset` walk past the corpse"
//! shape would be use-after-free by construction: the walk only ever visits
//! blocks that are still allocated, because deallocated ones are already off
//! the chain.
//!
//! # Chain mechanics
//!
//! * `allocate` head-pushes onto the live chain (`live_head`), threading
//!   `pprev` through either the arena's `live_head` slot or the previous
//!   head's `next` field — the tail slot `live_tail` only tracks where the
//!   chain *ends* so the retain splice stays O(1).
//! * `deallocate` unlinks through the block's own `pprev` (updating the
//!   successor's `pprev`, and `live_tail` when the live tail was removed)
//!   and frees the block. Quarantined blocks are unlinked the same way — a
//!   retained block whose caller frees it later through a stale handle is
//!   handled by exactly the same code path (the ruling's defensive layer).
//! * `reset_retain_with_limit` keeps the whole live chain when its accounted
//!   footprint fits under the limit by splicing it onto the quarantine list
//!   in O(1) — three pointer writes plus one boundary `pprev` fix, no
//!   per-block work. Over the limit it frees both chains and returns false
//!   (all-or-nothing, matching the mimalloc backend). The next full `reset`
//!   frees the quarantine too.
//! * `allocated_bytes` is the arena's `live_bytes` counter — incremented by
//!   `allocate` with the request size, decremented by `deallocate` with the
//!   layout's size, zeroed wholesale whenever both chains are freed. No size
//!   lives in the header because bulk-free does not need per-block sizes to
//!   hand memory back.
//!
//! # Thread contract (narrowed vs `mi_free`, flagged for the closure survey)
//!
//! `mi_free` was safe from any thread even while the owning thread kept
//! allocating. Here `deallocate` performs an intrusive unlink through
//! non-atomic header fields, so a `deallocate` racing an `allocate`/`reset`
//! on the same arena would race the chain. The enforced contract is:
//! `allocate`/`reset` on the constructing thread only (debug stamp, same as
//! the mimalloc backend), `deallocate` from any thread/any handle while the
//! arena is quiescent — the worker-frees-after-parse-yields pattern. If a
//! consumer needs interleaved owner-alloc + worker-free, that is a real
//! design change (chain locks defeat the arena's purpose) and goes back to
//! the user, not into this file silently.

use crate::core_alloc::{AllocError, Allocator, Layout};
use core::cell::Cell;
use core::mem::MaybeUninit;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(debug_assertions)]
use core::sync::atomic::AtomicU64;

use crate::fallback::C_ALLOCATOR;

// ── Block records ─────────────────────────────────────────────────────────

/// The block's own permanent record — the ownership link that makes
/// cross-arena/cross-handle free work without any lookup table. 24 bytes,
/// `repr(C)` so the field order is contractual; the header lives at the
/// front of a single-allocation block and in its own tiny malloc for
/// dual-allocation blocks, and dies with the block in both cases.
#[repr(C)]
struct Block {
    /// Chain link. While the block is live this points at the next-live
    /// block of the owner's chain (or null at the tail); the chain head is
    /// [`StdArena::live_head`] on the live chain and
    /// [`StdArena::quarantine_head`] on the quarantine chain.
    next: *mut Block,
    /// Address of the `next` slot that points at this block — the arena's
    /// `live_head`/`quarantine_head` slot or the predecessor block's `next`
    /// field. `*pprev = next` unlinks in O(1) with no predecessor walk; the
    /// successor's `pprev` is re-threaded to this block's `pprev` in the
    /// same step.
    pprev: *mut *mut Block,
    /// `null`: single allocation — the header is the front of a
    /// `malloc(24 + size)` block and the user pointer is `base + 24`.
    /// Non-null: dual allocation (requested align > 8) — the malloc base of
    /// the payload storage block; the user pointer sits at
    /// `align_up(storage + 8, align)` inside it and the storage word at
    /// `user - 8` holds this base again, so a bare user pointer can find the
    /// header. Both allocations are freed together.
    payload: *mut u8,
}

const _: () = assert!(core::mem::size_of::<Block>() == 24);

/// Bytes of header in front of a single-allocation block's user bytes.
const HEADER_LEN: usize = 24;

// ── Debug-only thread-ownership guard (same shape as MimallocArena's) ─────

#[cfg(debug_assertions)]
#[inline]
fn debug_thread_stamp() -> u64 {
    // Same contract as `MimallocArena::debug_thread_stamp`: a nonzero
    // per-thread-unique id for the same-thread alloc assert, without pulling
    // `bun_threading`/`bun_safety` (which sit above tier-0) into a cycle.
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::thread_local!(static ID: u64 = NEXT.fetch_add(1, Ordering::Relaxed));
    ID.with(|id| *id)
}

/// Std-backend arena: independent per-block libc allocations carrying their
/// own intrusive chain record, bulk-freed on `reset`/`Drop`.
///
/// Drop-in structural twin of [`crate::MimallocArena`] — same constructor and
/// management surface, same `Allocator for &StdArena` impl, same
/// `StdAllocator` vtable shape — so the `pub type Arena = …` swap and the
/// `ArenaString`/`ArenaVecExt`/`vec_from_iter_in`/`transfer_arena` consumers
/// re-point without semantic drift.
pub struct StdArena {
    /// `true` when this arena bulk-frees its blocks on `reset`/`Drop`.
    /// `false` for [`Self::borrowing_default`] — allocations live until
    /// individually freed (or process exit), `reset` is forbidden, `Drop`
    /// frees nothing. Matches the `borrowing_default()`/`mi_heap_main()`
    /// shape of `MimallocArena`.
    owns: bool,
    /// Head of the live chain — the arena's live set. Chained ⇒ not yet
    /// deallocated; `reset`/`Drop` free exactly this list. Mutated by the
    /// constructing thread (`allocate`/`reset`/`Drop`) and by `deallocate`'s
    /// unlink (any thread/handle while quiescent) — see the thread contract
    /// in the module docs.
    live_head: Cell<*mut Block>,
    /// Address of the `next` slot a new live block's chain position ends at:
    /// `&live_head` while the chain is empty, else the tail block's `next`
    /// field. Only exists so the retain splice and the empty-chain push are
    /// O(1); head-pushes leave it alone because pushing at the head never
    /// changes where the chain ends.
    live_tail: Cell<*mut *mut Block>,
    /// Head of the quarantine chain: whole live chains retained across a
    /// `reset_retain_with_limit`. These blocks are still logically live —
    /// they are freed by the next full `reset`, or earlier if their caller
    /// frees them through a (possibly stale) handle, which unlinks them from
    /// this chain exactly like a live-block free.
    quarantine_head: Cell<*mut Block>,
    /// Sum of request sizes over all blocks still held (live chain +
    /// quarantine). The `allocated_bytes()` figure; decremented by
    /// `deallocate` with the layout's size and zeroed wholesale whenever
    /// both chains are freed (bulk-free never needs per-block sizes).
    /// Relaxed is enough: an accounting value with no ordering-dependent
    /// reader.
    live_bytes: AtomicUsize,
    /// Debug-only: thread that constructed (or last `reset`) this arena.
    /// `0` = `borrowing_default()`, whose allocations are plain libc calls
    /// and thread-safe, so the same-thread assert is disabled.
    #[cfg(debug_assertions)]
    owning_thread: AtomicU64,
}

// SAFETY: mirrors `MimallocArena`: the arena may be *moved* into another
// thread. Allocate/reset stay on the constructing thread — enforced by the
// debug `owning_thread` stamp, exactly like the Zig `ThreadLock` the mimalloc
// backend mirrored. `Drop`/`reset` take `&mut self` and are therefore
// exclusive by construction.
unsafe impl Send for StdArena {}

// SAFETY: `&StdArena` is shared across threads so worker tasks can
// `deallocate` blocks they produced — through any handle, while the arena is
// quiescent (the thread contract in the module docs). The dealloc path's
// only cross-block touch is the unlink through `pprev`, and the remaining
// arena state it can reach is the `live_tail` repair, both valid under that
// contract. The hazard the mimalloc backend did NOT have — dealloc racing
// allocate on the same arena — is excluded by the contract and flagged in
// the module docs; if a consumer needs it, that is a user-level design
// decision, not a silent fix here.
unsafe impl Sync for StdArena {}

impl Default for StdArena {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl StdArena {
    /// `MimallocArena::new()` parity — a fresh, owning arena.
    #[inline]
    pub fn new() -> Self {
        Self {
            owns: true,
            live_head: Cell::new(ptr::null_mut()),
            live_tail: Cell::new(ptr::null_mut()),
            quarantine_head: Cell::new(ptr::null_mut()),
            live_bytes: AtomicUsize::new(0),
            #[cfg(debug_assertions)]
            owning_thread: AtomicU64::new(debug_thread_stamp()),
        }
    }

    /// Alias for [`Self::new`] — matches the Zig spelling kept by
    /// `MimallocArena::init`.
    #[inline]
    pub fn init() -> Self {
        Self::new()
    }

    /// `MimallocArena::borrowing_default()` parity — an arena that owns
    /// nothing: allocations are plain libc blocks freed individually (or at
    /// process exit), `Drop` is a no-op for payloads, and `reset` is
    /// forbidden (debug assert, mirroring "would destroy `mi_heap_main`").
    #[inline]
    pub fn borrowing_default() -> Self {
        Self {
            owns: false,
            live_head: Cell::new(ptr::null_mut()),
            live_tail: Cell::new(ptr::null_mut()),
            quarantine_head: Cell::new(ptr::null_mut()),
            live_bytes: AtomicUsize::new(0),
            #[cfg(debug_assertions)]
            owning_thread: AtomicU64::new(0),
        }
    }

    /// Debug-only check that the calling thread is the one that constructed
    /// (or last `reset`) this arena — the same-thread half of the `Sync`
    /// contract. `borrowing_default()` arenas stamp `0` and skip it.
    #[inline(always)]
    fn assert_owning_thread(&self) {
        #[cfg(debug_assertions)]
        {
            let owner = self.owning_thread.load(Ordering::Relaxed);
            if owner == 0 {
                return;
            }
            let cur = debug_thread_stamp();
            debug_assert_eq!(
                owner, cur,
                "StdArena: allocation on thread {cur}, but arena is owned by \
                 thread {owner} (arena alloc paths are single-threaded)"
            );
        }
    }

    /// Address of this arena's `live_head` slot, as the chain code sees it.
    #[inline]
    fn live_head_slot(&self) -> *mut *mut Block {
        ptr::from_ref(&self.live_head).cast_mut().cast::<*mut Block>()
    }

    /// Address of this arena's `quarantine_head` slot.
    #[inline]
    fn quarantine_head_slot(&self) -> *mut *mut Block {
        ptr::from_ref(&self.quarantine_head)
            .cast_mut()
            .cast::<*mut Block>()
    }

    /// `MimallocArena::allocated_bytes` parity — bytes currently held by this
    /// arena: request sizes of the live chain plus quarantined chains. A
    /// maintained counter, not a walk (the mimalloc backend walked heap
    /// areas; the chain makes counting the natural form here). Feeds GC
    /// `estimatedSize` reporting and the retain decision. Note: a block
    /// freed through a *different* arena's handle decrements that handle's
    /// counter (the free path is deliberately arena-agnostic); the owning
    /// arena's figure is therefore only exact until a cross-arena free
    /// happens, and every bulk-free zeroes it wholesale, so retain/reset
    /// decisions stay safe under either sign of the skew.
    #[inline]
    pub fn allocated_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::Relaxed)
    }

    /// Destroy every held block — live chain and quarantine — and start a
    /// fresh cycle. `MimallocArena::reset` parity (`mi_heap_destroy` +
    /// `mi_heap_new` becomes: free both chains, zero the counter). Pointers
    /// previously returned by this arena are invalidated.
    ///
    /// The walk visits only blocks that are still allocated: deallocated
    /// blocks unlinked themselves at `deallocate` time and are gone from the
    /// chain, so there is nothing to skip and nothing to double-free.
    #[cold]
    #[inline(never)]
    pub fn reset(&mut self) {
        debug_assert!(
            self.owns,
            "StdArena::reset() on a borrowing_default() arena — its blocks \
             live for the process lifetime and must not be bulk-freed"
        );
        // SAFETY: `&mut self` — no allocate/reset can run concurrently (the
        // single-thread-alloc contract) and deallocate is excluded by the
        // quiescence contract, so both chains are quiescent.
        unsafe {
            self.free_chain(self.live_head.take());
            self.free_chain(self.quarantine_head.take());
        }
        self.live_bytes.store(0, Ordering::Relaxed);
        self.live_tail.set(self.live_head_slot());
        // Re-stamp the debug thread-lock: an arena Send-moved to a worker and
        // reset there may allocate on that worker (same as `MimallocArena`).
        #[cfg(debug_assertions)]
        self.owning_thread
            .store(debug_thread_stamp(), Ordering::Relaxed);
    }

    /// `MimallocArena::reset_retain_with_limit` parity — keep the cycle's
    /// allocations when the accounted footprint fits under `limit`, reclaim
    /// everything otherwise.
    ///
    /// Under the limit: the entire live chain is spliced onto the quarantine
    /// list in O(1) — the head crosses over, the chain's tail is re-threaded
    /// onto whatever was quarantined before, and the only block header
    /// touched is the boundary `pprev`. Returns `true`. The retained blocks
    /// are still logically live: the next full [`Self::reset`] frees them,
    /// and a caller freeing one earlier through any handle unlinks it from
    /// the quarantine exactly like a live-block free.
    ///
    /// Over the limit: both chains are freed and `false` is returned —
    /// all-or-nothing, matching the mimalloc backend's cap-gated
    /// `mi_heap_destroy` recycle.
    #[inline]
    pub fn reset_retain_with_limit(&mut self, limit: usize) -> bool {
        if !self.owns {
            // Parity with the mimalloc backend: a borrowing arena falls
            // through to `reset`, whose debug assert flags the misuse.
            self.reset();
            return false;
        }
        if self.allocated_bytes() <= limit {
            let live = self.live_head.get();
            if !live.is_null() {
                // Splice the whole live chain onto the front of the
                // quarantine chain. Only two headers change: the first live
                // block (its `pprev` moves to the quarantine head slot) and
                // the boundary between the moved chain and whatever was
                // quarantined before.
                let live_tail_slot = self.live_tail.replace(self.live_head_slot());
                self.live_head.set(ptr::null_mut());
                // SAFETY: `live` and the old quarantine head are chain
                // heads; both chains are quiescent under `&mut self`.
                unsafe {
                    (*live).pprev = self.quarantine_head_slot();
                    let old_quarantine = self.quarantine_head.get();
                    *live_tail_slot = old_quarantine;
                    if !old_quarantine.is_null() {
                        (*old_quarantine).pprev = live_tail_slot;
                    }
                }
                self.quarantine_head.set(live);
            }
            // `&mut self` proves exclusivity; re-stamp like `reset` does.
            #[cfg(debug_assertions)]
            self.owning_thread
                .store(debug_thread_stamp(), Ordering::Relaxed);
            return true;
        }

        // Over limit: all-or-nothing reclaim — free both chains.
        // SAFETY: `&mut self`, same quiescence argument as `reset`.
        unsafe {
            self.free_chain(self.live_head.take());
            self.free_chain(self.quarantine_head.take());
        }
        self.live_bytes.store(0, Ordering::Relaxed);
        self.live_tail.set(self.live_head_slot());
        #[cfg(debug_assertions)]
        self.owning_thread
            .store(debug_thread_stamp(), Ordering::Relaxed);
        false
    }

    /// `MimallocArena::gc` parity — no-op. The mimalloc backend collected
    /// empty pages out of an opaque `mi_heap_t`; there is no such opaque
    /// heap here. Per-block libc frees have already returned every
    /// deallocated byte, and retained blocks are still logically live, so
    /// there is nothing left for a collect pass to reclaim.
    #[inline]
    pub fn gc(&self) {}

    /// `MimallocArena::helpCatchMemoryIssues` parity — no-op. The mimalloc
    /// version force-collected this heap *and* the global mimalloc state to
    /// surface use-after-free early; both are libc allocations here, and
    /// ASAN (the consumer of that hook) instruments libc directly.
    #[inline]
    pub fn help_catch_memory_issues(&self) {}

    /// `MimallocArena::resize_in_place` parity — always `false`.
    ///
    /// Precedent: `basic.rs`'s vtable `resize` returns `false` under ASAN
    /// (and, after the std-backend migration, unconditionally — plain libc
    /// has no in-place expand), and a `resize → false` is a legal allocator
    /// contract: callers fall back to remap/realloc. With independent
    /// per-block allocations there is no neighbor-free slot to expand into,
    /// so the honest answer is always "cannot resize in place".
    #[inline]
    pub fn resize_in_place(&self, _ptr: NonNull<u8>, _old_len: usize, _new_len: usize) -> bool {
        false
    }

    /// Grow/shrink primitive: allocate the new block, copy the
    /// `min(old, new)` prefix, then unlink-and-free the old block through
    /// its own header — the block-level shape of the `transfer_arena`
    /// contract line "allocs on dst, memcpy, then mi_free(ptr)". Returns
    /// null on allocation failure with the old block left untouched (the
    /// `mi_heap_realloc_aligned` contract).
    fn remap(&self, ptr: NonNull<u8>, old_len: usize, new_len: usize, align: usize) -> *mut u8 {
        let new = self.alloc_block(new_len, align, false);
        if new.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `new` is fresh from `alloc_block` and does not overlap the
        // still-live old payload; copying `min(old, new)` bytes stays inside
        // both allocations.
        unsafe {
            ptr::copy_nonoverlapping(ptr.as_ptr(), new, old_len.min(new_len));
            self.free_payload(ptr, old_len);
        }
        new
    }

    // ── Allocation core ───────────────────────────────────────────────────

    /// Allocate a payload of `size` bytes aligned to `align` (raw; the
    /// `Allocator` layer wraps this in `Result`). Single-allocation blocks
    /// (align ≤ 8) carry the header in front of the payload; dual-allocation
    /// blocks (align > 8) get a separate manually-aligned storage block.
    /// Returns the *user* pointer, or null on failure.
    #[inline]
    fn alloc_block(&self, size: usize, align: usize, zeroed: bool) -> *mut u8 {
        self.assert_owning_thread();
        if align <= 8 {
            // Single allocation: [Block 24 B][user bytes]. `base + 24` is
            // 8-aligned, which covers every align ≤ 8; malloc's own 16-byte
            // alignment is what makes the header address clean.
            let total = HEADER_LEN + size;
            let base = self.raw_calloc(total, zeroed);
            if base.is_null() {
                return ptr::null_mut();
            }
            // SAFETY: `base` is `total` bytes from the plain-libc path;
            // header + user bytes fit by construction.
            unsafe { (*base.cast::<Block>()).payload = ptr::null_mut() };
            // SAFETY: fresh header, this thread owns the chain.
            unsafe { self.link_block(base.cast::<Block>()) };
            self.live_bytes.fetch_add(size, Ordering::Relaxed);
            // SAFETY: fixed displacement over a live allocation.
            unsafe { base.add(HEADER_LEN) }
        } else {
            // Dual allocation: a 24 B header block plus a manually-aligned
            // storage block. Both are plain mallocs, so every free in this
            // module is a plain `free` — nothing has to remember an
            // alignment to pick the matching free entry point.
            let storage_len = align + size; // ≥ align bytes of align-up slop + payload
            let storage = self.raw_calloc(storage_len, zeroed);
            if storage.is_null() {
                return ptr::null_mut();
            }
            let header = self.raw_alloc(HEADER_LEN).cast::<Block>();
            if header.is_null() {
                // SAFETY: `storage` was just returned; freed so the failed
                // allocation leaves nothing behind.
                unsafe { self.raw_free(storage) };
                return ptr::null_mut();
            }
            // SAFETY: `storage` holds `storage_len ≥ 8 + align` bytes; the
            // user pointer lands at `storage + align` (aligned by
            // construction, since `storage` is 16-aligned and `align` is a
            // multiple of 16 for every align > 8), keeping the back-link
            // word at `user - 8` inside the allocation.
            let user = unsafe { storage.add(align) };
            // SAFETY: both blocks are live allocations from the paths above.
            unsafe {
                (*header).payload = storage;
                ptr::write(user.sub(8).cast::<*mut Block>(), header);
                self.link_block(header);
            }
            self.live_bytes.fetch_add(size, Ordering::Relaxed);
            user
        }
    }

    /// Thread a freshly-written header block onto the live chain: it takes
    /// over the head slot (its `pprev` points there), and the previous head
    /// — if any — re-threads its `pprev` to the new block's `next` field.
    /// Pushing at the head never moves the chain's end, so `live_tail` only
    /// changes when the chain was empty.
    ///
    /// # Safety
    /// `block` must be a live header with `payload` already written, not
    /// visible to any other thread yet.
    unsafe fn link_block(&self, block: *mut Block) {
        // SAFETY: caller contract — live header, this thread owns the chain.
        unsafe {
            let prev_head = self.live_head.get();
            (*block).next = prev_head;
            (*block).pprev = self.live_head_slot();
            if !prev_head.is_null() {
                // The old head now sits behind the new one.
                (*prev_head).pprev = ptr::addr_of_mut!((*block).next);
            } else {
                // Empty chain: the push also becomes the chain's end.
                self.live_tail.set(ptr::addr_of_mut!((*block).next));
            }
            self.live_head.set(block);
        }
    }

    /// Plain-libc allocation. Every block in this module is a plain
    /// malloc-family block — aligned payloads are aligned *inside* a plain
    /// malloc block — so every matching free is a plain `free` and nothing
    /// ever has to remember an alignment.
    #[inline]
    fn raw_alloc(&self, total: usize) -> *mut u8 {
        match C_ALLOCATOR.raw_alloc(total, crate::Alignment::from_byte_units(16), 0) {
            Some(p) => p,
            None => ptr::null_mut(),
        }
    }

    /// Zeroed variant: `calloc` (the zeroing comes from the CRT, so no
    /// second pass is needed).
    #[inline]
    fn raw_calloc(&self, total: usize, zeroed: bool) -> *mut u8 {
        if zeroed {
            // SAFETY: `total > 0`; `calloc` with nmemb=1 has no overflow.
            unsafe { libc::calloc(1, total).cast() }
        } else {
            self.raw_alloc(total)
        }
    }

    /// Plain libc free.
    ///
    /// # Safety
    /// `base` must be a live allocation from [`Self::raw_alloc`]/[`Self::raw_calloc`].
    #[inline]
    unsafe fn raw_free(&self, base: *mut u8) {
        // SAFETY: caller contract — matching plain-libc allocation.
        unsafe { libc::free(base.cast()) }
    }

    /// Unlink a block from whichever chain it sits on (live or quarantine)
    /// and repair the bookkeeping: the predecessor slot takes over `next`,
    /// the successor re-threads its `pprev`, and the live chain's tail slot
    /// is moved back when the removed block was the live chain's end. The
    /// quarantine chain keeps no tail slot — its tail is only ever consumed
    /// by a full walk.
    ///
    /// # Safety
    /// `node` must be a live header currently chained on this arena, and the
    /// arena must be quiescent (no concurrent allocate/reset).
    unsafe fn unlink_block(&self, node: *mut Block) {
        // SAFETY: caller contract — chained, quiescent.
        unsafe {
            let next = (*node).next;
            *(*node).pprev = next;
            if !next.is_null() {
                (*next).pprev = (*node).pprev;
            }
            if self.live_tail.get() == ptr::addr_of_mut!((*node).next) {
                // `node` was the live chain's tail: its `pprev` slot now
                // holds the chain's end.
                self.live_tail.set((*node).pprev);
            }
        }
    }

    /// Free a user pointer: recover the header through the word at
    /// `user - 8` (null ⇒ single allocation, header at `user - 24`;
    /// non-null ⇒ dual allocation, and the word *is* the header block's
    /// base), unlink the block through its own header, and free every
    /// allocation it owns.
    ///
    /// Sound from any thread and through any `StdArena` handle while the
    /// arena is quiescent: the ownership link is the block header itself —
    /// the structural form of `transfer_arena`'s cross-arena contract.
    ///
    /// # Safety
    /// `ptr` must be a live user pointer produced by *some* `StdArena`, and
    /// the arena owning it must be quiescent. Double-free detection is the
    /// caller's responsibility, exactly as it was for `mi_free`.
    unsafe fn free_payload(&self, ptr: NonNull<u8>, size: usize) {
        let user = ptr.as_ptr();
        // SAFETY: caller contract — the word immediately before the user
        // pointer belongs to this block: the header's `payload` field for a
        // single allocation, the storage back-link for a dual one.
        let word = unsafe { ptr::read(user.sub(8).cast::<*mut Block>()) };
        let (header, storage) = if word.is_null() {
            // Single allocation: the header is the block front.
            // SAFETY: same caller contract — `user - HEADER_LEN` is the
            // live block base.
            let base = unsafe { user.sub(HEADER_LEN).cast::<Block>() };
            (base, ptr::null_mut())
        } else {
            // Dual allocation: the word is the header block's base, and the
            // header's `payload` field is the aligned storage's base.
            let header = word;
            // SAFETY: `header` is the live header block for this payload.
            let storage = unsafe { (*header).payload };
            (header, storage)
        };
        // SAFETY: the block is chained and the arena is quiescent.
        unsafe { self.unlink_block(header) };
        // Accounting: saturate at zero — a cross-handle free runs against the
        // *calling* handle's counter, which may hold less than this block's
        // size; the owning arena's figure self-heals at the next wholesale
        // zeroing (reset / over-limit reclaim / Drop).
        let _ = self
            .live_bytes
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(size));
        // SAFETY: matching plain-libc frees — the storage first (it may
        // share the block in the single case, where it is null), then the
        // header.
        unsafe {
            if !storage.is_null() {
                self.raw_free(storage);
            }
            self.raw_free(header.cast::<u8>());
        }
    }

    /// Free a whole chain from `head` (the caller `take()`s the head first).
    /// Every block on a chain is still allocated — deallocated blocks left
    /// the chain at `deallocate` — so this is a straight free walk with
    /// nothing to skip.
    ///
    /// # Safety
    /// The caller must hold exclusive access to the arena (`&mut self`), so
    /// the chain is quiescent during the walk.
    unsafe fn free_chain(&self, head: *mut Block) {
        let mut cur = head;
        while !cur.is_null() {
            // SAFETY: chain blocks are live allocations under `&mut self`.
            let node = unsafe { &*cur };
            let next = node.next;
            // SAFETY: still-held block; freed exactly once by this walk.
            unsafe {
                if !node.payload.is_null() {
                    self.raw_free(node.payload);
                }
                self.raw_free(cur.cast::<u8>());
            }
            cur = next;
        }
    }

    // ── bumpalo-compatible surface ────────────────────────────────────────
    // (retained verbatim in shape from the mimalloc twin)
// ── bumpalo-compatible surface ────────────────────────────────────────
    // Same shape as `MimallocArena`'s so `pub type Arena = StdArena` is
    // source-compatible with the bumpalo alias this line descended from.
    // Borrows are reclaimed on `reset`/`Drop` (or earlier via the
    // `Allocator` impl's `deallocate`).

    /// `bumpalo::Bump::alloc_layout` parity.
    #[inline]
    pub fn alloc_layout(&self, layout: Layout) -> NonNull<u8> {
        let p = self.alloc_block(layout.size(), layout.align(), false);
        NonNull::new(p).unwrap_or_else(|| crate::out_of_memory())
    }

    /// `bumpalo::Bump::alloc` parity — move `val` into the arena.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc<T>(&self, val: T) -> &mut T {
        let p = self.alloc_layout(Layout::new::<T>()).cast::<T>();
        // SAFETY: `p` is non-null, properly aligned, and points to at least
        // `size_of::<T>()` uninitialized bytes owned by this arena.
        unsafe {
            p.as_ptr().write(val);
            &mut *p.as_ptr()
        }
    }

    /// `bumpalo::Bump::alloc_str` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_str(&self, s: &str) -> &mut str {
        let bytes = self.alloc_slice_copy(s.as_bytes());
        // SAFETY: copied from valid UTF-8.
        unsafe { core::str::from_utf8_unchecked_mut(bytes) }
    }

    /// `bumpalo::Bump::alloc_slice_copy` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_copy<T: Copy>(&self, src: &[T]) -> &mut [T] {
        let layout = Layout::for_value(src);
        let dst = self.alloc_layout(layout).cast::<T>();
        // SAFETY: `dst` is freshly allocated, aligned for `T`, sized for
        // `src.len()` elements; ranges do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_ptr(), src.len());
            core::slice::from_raw_parts_mut(dst.as_ptr(), src.len())
        }
    }

    /// `bumpalo::Bump::alloc_slice_clone` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_clone<T: Clone>(&self, src: &[T]) -> &mut [T] {
        self.alloc_slice_fill_iter(src.iter().cloned())
    }

    /// `bumpalo::Bump::alloc_slice_fill_default` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_fill_default<T: Default>(&self, len: usize) -> &mut [T] {
        self.alloc_slice_fill_with(len, |_| T::default())
    }

    /// `bumpalo::Bump::alloc_slice_fill_copy` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_fill_copy<T: Copy>(&self, len: usize, value: T) -> &mut [T] {
        self.alloc_slice_fill_with(len, |_| value)
    }

    /// `bumpalo::Bump::alloc_slice_fill_with` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_fill_with<T, F>(&self, len: usize, mut f: F) -> &mut [T]
    where
        F: FnMut(usize) -> T,
    {
        let layout = Layout::array::<T>(len).unwrap_or_else(|_| crate::out_of_memory());
        let dst = self.alloc_layout(layout).cast::<T>();
        // SAFETY: `dst` is aligned for `T` and sized for `len` elements. We
        // initialize every slot before forming the slice. If `f` panics the
        // partially-initialized prefix leaks into the arena (reclaimed on
        // `reset`/`Drop`) — same behavior as bumpalo.
        unsafe {
            for i in 0..len {
                dst.as_ptr().add(i).write(f(i));
            }
            core::slice::from_raw_parts_mut(dst.as_ptr(), len)
        }
    }

    /// `bumpalo::Bump::alloc_slice_fill_iter` parity.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_slice_fill_iter<T, I>(&self, iter: I) -> &mut [T]
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        self.alloc_slice_fill_with(len, |_| {
            iter.next()
                .expect("ExactSizeIterator under-reported length")
        })
    }

    /// Allocate an uninitialized `[MaybeUninit<T>; len]` slice.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_uninit_slice<T>(&self, len: usize) -> &mut [MaybeUninit<T>] {
        let layout = Layout::array::<T>(len).unwrap_or_else(|_| crate::out_of_memory());
        let dst = self.alloc_layout(layout).cast::<MaybeUninit<T>>();
        // SAFETY: `MaybeUninit<T>` has the same layout as `T` and imposes no
        // initialization invariant.
        unsafe { core::slice::from_raw_parts_mut(dst.as_ptr(), len) }
    }

    // ── StdAllocator vtable bridge ────────────────────────────────────────

    /// `MimallocArena::std_allocator` parity — erase to the fat
    /// `{ptr, vtable}` handle for code threading the Zig-style allocator.
    /// `ctx` is the `*const StdArena` so the thunks can reach the arena;
    /// `is_instance` compares vtable addresses.
    #[inline]
    pub fn std_allocator(&self) -> crate::StdAllocator {
        crate::StdAllocator {
            ptr: ptr::from_ref(self).cast_mut().cast(),
            vtable: &HEAP_ALLOCATOR_VTABLE,
        }
    }

    /// `MimallocArena::is_instance` parity — does this handle dispatch
    /// through this module's vtable? (The mimalloc backend also matched its
    /// process-global vtable; that form has no analogue here — the global
    /// default allocator is the plain libc `basic::C_ALLOCATOR`, a different
    /// module's identity, and an arena check must not claim it.)
    #[inline]
    pub fn is_instance(alloc: &crate::StdAllocator) -> bool {
        core::ptr::eq(alloc.vtable, &raw const HEAP_ALLOCATOR_VTABLE)
    }

    /// Vtable addresses this module hands out, for alloc-vtable registration
    /// consumers (the mimalloc backend exposed two: per-heap + global; the
    /// std backend has only the per-arena form).
    #[inline]
    pub fn std_vtables() -> [&'static crate::AllocatorVTable; 1] {
        [&HEAP_ALLOCATOR_VTABLE]
    }
}

impl Drop for StdArena {
    /// LIFECYCLE PIN (wave #45 closure survey, S4): unlike the deleted
    /// `MimallocArena` — where an ownership transfer abdicated whole *pages*
    /// and transferred blocks survived the arena — `Drop` here frees EVERY
    /// live block, including blocks moved out via `transfer_arena`'s
    /// endorsement. The bundler worker arena's X1-endorsed data
    /// (`symbols`/`parts`/`import_records`) is therefore freed at
    /// `Worker::deinit`'s arena drop; the current flows are safe because
    /// (CLI) the bundle object is abandoned wholesale after deinit and never
    /// dereferenced, and (dev server) the pool outlives the pass so no deinit
    /// runs mid-flight. Any new flow that reads the graph AFTER deinit must
    /// re-audit this contract first.
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: `&mut self`; same quiescence argument as `reset`.
            unsafe {
                self.free_chain(self.live_head.take());
                self.free_chain(self.quarantine_head.take());
            }
            self.live_bytes.store(0, Ordering::Relaxed);
        }
        // A borrowing arena's payloads live until individually freed (or
        // process exit) — the semantics of dropping a `borrowing_default()`
        // handle over `mi_heap_main`. There is no arena-owned ancillary
        // storage to release: headers die with their blocks.
    }
}

// ── core::alloc::Allocator ────────────────────────────────────────────────
//
// On `&StdArena` (not the owned value) so `Vec<T, &'a StdArena>` borrows the
// arena for `'a` — the bumpalo-shaped contract `ArenaVec`/`BabyVec` are
// built on. This impl is what `ArenaString`/`ArenaVecExt`/`vec_from_iter_in`/
// `transfer_arena` sit on; consumers need no changes beyond the type swap.

/// Wrap a raw user pointer in the `Result<NonNull<[u8]>, AllocError>` shape
/// the `Allocator` trait wants (`#[inline(always)]`, hot path — same helper
/// as the mimalloc backend's).
#[inline(always)]
fn alloc_result(p: *mut u8, size: usize) -> Result<NonNull<[u8]>, AllocError> {
    NonNull::new(p)
        .map(|p| NonNull::slice_from_raw_parts(p, size))
        .ok_or(AllocError)
}

// SAFETY:
// - `allocate` returns `size` user bytes aligned to `layout.align()`, backed
//   by an independent libc allocation; `deallocate` frees exactly that
//   allocation, immediately (per-allocation free is the point of this
//   backend); `grow`/`shrink` relocate with the `min(old, new)` prefix
//   preserved and the old block freed.
// - `reset`/`Drop` bulk-free every block the caller leaked.
// - Cloned `&StdArena` handles refer to the same instance, and a handle from
//   a *different* instance may free this backend's blocks (independent
//   allocations carry their own prefix) — `transfer_arena`'s contract.
unsafe impl Allocator for &StdArena {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        alloc_result(
            self.alloc_block(layout.size(), layout.align(), false),
            layout.size(),
        )
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        alloc_result(
            self.alloc_block(layout.size(), layout.align(), true),
            layout.size(),
        )
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: caller contract — `ptr` is a live payload of this (or a
        // sibling) arena with `layout`; see `free_payload`.
        unsafe { self.free_payload(ptr, layout.size()) }
    }

    #[inline]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        alloc_result(
            self.remap(ptr, old.size(), new.size(), new.align()),
            new.size(),
        )
    }

    #[inline]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let p = self.remap(ptr, old.size(), new.size(), new.align());
        let p = NonNull::new(p).ok_or(AllocError)?;
        // SAFETY: `p` holds `new.size()` bytes; the `[old, new)` tail is
        // freshly allocated and uninitialized.
        unsafe { ptr::write_bytes(p.as_ptr().add(old.size()), 0, new.size() - old.size()) };
        Ok(NonNull::slice_from_raw_parts(p, new.size()))
    }

    #[inline]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        alloc_result(
            self.remap(ptr, old.size(), new.size(), new.align()),
            new.size(),
        )
    }
}

// ── StdAllocator vtable ───────────────────────────────────────────────────

unsafe fn vtable_alloc(
    ctx: *mut core::ffi::c_void,
    len: usize,
    a: crate::Alignment,
    _ra: usize,
) -> *mut u8 {
    // SAFETY: `ctx` is the `*const StdArena` stashed by `std_allocator()`;
    // the `StdAllocator` borrow it came from is still live (Zig contract: an
    // allocator does not outlive its backing).
    let arena = unsafe { &*ctx.cast::<StdArena>() };
    arena.alloc_block(len, a.to_byte_units(), false)
}

unsafe fn vtable_resize(
    ctx: *mut core::ffi::c_void,
    _buf: &mut [u8],
    _a: crate::Alignment,
    _new_len: usize,
    _ra: usize,
) -> bool {
    // Same contract as `StdArena::resize_in_place` (the `basic.rs` ASAN
    // precedent): in-place resize is never available on this backend;
    // vtable users fall back to remap.
    let _ = ctx;
    false
}

unsafe fn vtable_remap(
    ctx: *mut core::ffi::c_void,
    buf: &mut [u8],
    a: crate::Alignment,
    new_len: usize,
    _ra: usize,
) -> *mut u8 {
    // SAFETY: see `vtable_alloc`.
    let arena = unsafe { &*ctx.cast::<StdArena>() };
    arena.remap(
        // SAFETY: `buf` is a live arena payload per the vtable contract.
        unsafe { NonNull::new_unchecked(buf.as_mut_ptr()) },
        buf.len(),
        new_len,
        a.to_byte_units(),
    )
}

unsafe fn vtable_free(
    ctx: *mut core::ffi::c_void,
    buf: &mut [u8],
    _a: crate::Alignment,
    _ra: usize,
) {
    // SAFETY: vtable contract — `buf` was allocated by this arena.
    unsafe {
        (*ctx.cast::<StdArena>()).free_payload(NonNull::new_unchecked(buf.as_mut_ptr()), buf.len())
    }
}

/// Per-arena thunks; `ctx` is the `*const StdArena` stashed by
/// `std_allocator()`. (`pub(crate)`: `std_vtables`/`is_instance` are the
/// public face, mirroring the mimalloc backend's vtable exposure.)
pub(crate) static HEAP_ALLOCATOR_VTABLE: crate::AllocatorVTable = crate::AllocatorVTable {
    alloc: vtable_alloc,
    resize: vtable_resize,
    remap: vtable_remap,
    free: vtable_free,
};

// ── ArenaVec helpers ──────────────────────────────────────────────────────

/// `bumpalo::collections::String` parity — a UTF-8 buffer backed by the
/// arena. Structural twin of the mimalloc backend's `ArenaString`, over
/// `&'a StdArena`.
pub struct ArenaString<'a> {
    buf: crate::core_alloc::AllocVec<u8, &'a StdArena>,
}

impl<'a> ArenaString<'a> {
    #[inline]
    pub fn new_in(arena: &'a StdArena) -> Self {
        Self {
            buf: crate::core_alloc::AllocVec::new_in(arena),
        }
    }
    #[inline]
    pub fn with_capacity_in(cap: usize, arena: &'a StdArena) -> Self {
        Self {
            buf: crate::core_alloc::AllocVec::with_capacity_in(cap, arena),
        }
    }
    #[inline]
    pub fn from_str_in(s: &str, arena: &'a StdArena) -> Self {
        let mut buf = crate::core_alloc::AllocVec::with_capacity_in(s.len(), arena);
        buf.extend_from_slice(s.as_bytes());
        Self { buf }
    }
    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: `buf` is only ever extended via `push_str`/`write_str`,
        // both of which append UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.buf) }
    }
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    /// `bumpalo::collections::String::into_bump_str` parity — leaks into the
    /// arena (reclaimed on `reset`/`Drop`).
    #[inline]
    pub fn into_bump_str(self) -> &'a str {
        let bytes = self.buf.into_bump_slice();
        // SAFETY: see `as_str`.
        unsafe { core::str::from_utf8_unchecked(bytes) }
    }
}

impl core::fmt::Write for ArenaString<'_> {
    #[inline]
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// Extension methods on `Vec<T, &StdArena>` covering the
/// `bumpalo::collections::Vec` API gaps (structural twin of the mimalloc
/// backend's `ArenaVecExt`).
pub trait ArenaVecExt<'a, T> {
    /// `bumpalo::collections::Vec::from_iter_in` parity.
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a StdArena) -> Self;
    /// `bumpalo::collections::Vec::into_bump_slice` parity — leaks into the
    /// arena (reclaimed on `reset`/`Drop`).
    fn into_bump_slice(self) -> &'a [T];
    /// `bumpalo::collections::Vec::into_bump_slice_mut` parity.
    fn into_bump_slice_mut(self) -> &'a mut [T];
    /// `bumpalo::collections::Vec::bump` parity — recover the backing arena.
    fn bump(&self) -> &'a StdArena;
}

impl<'a, T> ArenaVecExt<'a, T> for crate::core_alloc::AllocVec<T, &'a StdArena> {
    #[inline]
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a StdArena) -> Self {
        let iter = iter.into_iter();
        let (lo, _) = iter.size_hint();
        let mut v = crate::core_alloc::AllocVec::with_capacity_in(lo, arena);
        v.extend(iter);
        v
    }
    #[inline]
    fn into_bump_slice(self) -> &'a [T] {
        &*self.leak()
    }
    #[inline]
    fn into_bump_slice_mut(self) -> &'a mut [T] {
        self.leak()
    }
    #[inline]
    fn bump(&self) -> &'a StdArena {
        *self.allocator()
    }
}


// ── Self-tests ────────────────────────────────────────────────────────────
//
// Run in isolation: this file is wired into the crate by the closure segment;
// until then a scratch harness compiles it against faithful stubs of the
// crate-root items it names (see the S1-b report).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_alloc::AllocVec;

    fn assert_send_sync<T: Send + Sync>() {}

    /// Test-side alloc through the `&StdArena` Allocator impl.
    fn talloc(arena: &StdArena, size: usize, align: usize) -> NonNull<[u8]> {
        arena
            .allocate(Layout::from_size_align(size, align).unwrap())
            .unwrap()
    }

    /// Test-side free through the `&StdArena` Allocator impl.
    ///
    /// # Safety
    /// `p` must be a live payload of `arena` (or a sibling arena) with this
    /// size/align.
    unsafe fn tfree(arena: &StdArena, p: NonNull<[u8]>, size: usize, align: usize) {
        let ptr = NonNull::new(p.as_ptr().cast::<u8>()).unwrap();
        // SAFETY: caller contract — live payload with this size/align.
        unsafe { arena.deallocate(ptr, Layout::from_size_align(size, align).unwrap()) }
    }

    #[test]
    fn send_sync_static_asserts() {
        assert_send_sync::<StdArena>();
    }

    #[test]
    fn single_alloc_lifecycle_and_reuse_after_reset() {
        let mut arena = StdArena::new();
        // align 8 ⇒ the single-allocation hot path.
        let mut p = talloc(&arena, 64, 8);
        // SAFETY: live payload of this arena.
        unsafe {
            p.as_mut().fill(0xAB);
            tfree(&arena, p, 64, 8);
        }
        arena.reset();
        assert_eq!(arena.allocated_bytes(), 0);
        // The arena must be fully usable after a reset (fresh cycle).
        let b = talloc(&arena, 64, 8);
        assert_eq!(b.len(), 64);
        // SAFETY: live payload of this arena.
        unsafe { tfree(&arena, b, 64, 8) };
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn dual_alloc_lifecycle_align_16_and_32() {
        let mut arena = StdArena::new();
        // aligns 16 and 32 ⇒ dual-allocation paths (align > 8).
        let mut a = talloc(&arena, 100, 16);
        let mut b = talloc(&arena, 100, 32);
        // SAFETY: live payloads — write, verify isolation, free.
        unsafe {
            a.as_mut().fill(0x11);
            b.as_mut().fill(0x22);
            assert!(a.as_ref().iter().all(|&x| x == 0x11));
            assert!(b.as_ref().iter().all(|&x| x == 0x22));
            assert_eq!(arena.allocated_bytes(), 200);
            tfree(&arena, a, 100, 16);
            tfree(&arena, b, 100, 32);
        }
        assert_eq!(arena.allocated_bytes(), 0);
        arena.reset();
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn deallocate_is_immediate_real_free() {
        let arena = StdArena::new();
        let p = talloc(&arena, 128, 8);
        assert_eq!(arena.allocated_bytes(), 128);
        // SAFETY: live payload of this arena.
        unsafe { tfree(&arena, p, 128, 8) };
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn dealloc_unlink_keeps_reset_off_freed_memory() {
        let mut arena = StdArena::new();
        let mut live = Vec::new();
        for i in 0..16 {
            // Alternate single (align 8) and dual (align 16) paths.
            let p = talloc(&arena, 48, if i % 2 == 0 { 8 } else { 16 });
            if i % 3 == 0 {
                // SAFETY: fresh payload; exercise dealloc-before-reset,
                // including head nodes, middle nodes and tail nodes.
                unsafe { tfree(&arena, p, 48, if i % 2 == 0 { 8 } else { 16 }) };
            } else {
                live.push(p);
            }
        }
        assert_eq!(arena.allocated_bytes(), 16 * 48 - (6 * 48));
        arena.reset();
        assert_eq!(arena.allocated_bytes(), 0);
        // Freed blocks are off the chain: the reset walk never touched them
        // (the absence of a double-free/ASAN abort here is the assertion).
        drop(live);
    }

    #[test]
    fn head_and_tail_node_unlink() {
        let mut arena = StdArena::new();
        let a = talloc(&arena, 32, 8); // chain: A
        let b = talloc(&arena, 32, 8); // chain: B -> A (head-push)
        let c = talloc(&arena, 32, 8); // chain: C -> B -> A
        assert_eq!(arena.allocated_bytes(), 96);
        // Free the chain head (C) — its pprev points at the arena's live_head
        // slot; the general path must handle it like any other node.
        // SAFETY: live payloads.
        unsafe { tfree(&arena, c, 32, 8) };
        assert_eq!(arena.allocated_bytes(), 64);
        // Free the chain tail (A) — exercises the live_tail repair.
        // SAFETY: live payload.
        unsafe { tfree(&arena, a, 32, 8) };
        assert_eq!(arena.allocated_bytes(), 32);
        // Free the last remaining node (B) — head and tail at once.
        // SAFETY: live payload.
        unsafe { tfree(&arena, b, 32, 8) };
        assert_eq!(arena.allocated_bytes(), 0);
        // The arena is still fully usable after emptying by hand.
        let d = talloc(&arena, 32, 8);
        // SAFETY: live payload.
        unsafe { tfree(&arena, d, 32, 8) };
        arena.reset();
    }

    #[test]
    fn cross_arena_dealloc_via_transfer_contract() {
        let a = StdArena::new();
        let b = StdArena::new();
        let p = talloc(&a, 64, 16);
        // Free through a different arena's handle — `transfer_arena`'s
        // cross-arena contract. The ownership link is the block header
        // itself, so handle B can unlink and free it; `a`'s reset must not
        // touch the freed block afterwards.
        // SAFETY: `p` is a live arena payload; cross-handle free is the
        // contract under test.
        unsafe { tfree(&b, p, 64, 16) };
        let mut a = a;
        a.reset();
        // Same through the StdAllocator vtable of a third handle.
        let c = StdArena::new();
        let q = talloc(&a, 64, 16);
        let handle = c.std_allocator();
        // Vtable contract — `q` is an arena payload; the std-backend free
        // path is arena-agnostic.
        // SAFETY: `q` is a live 64-byte payload of `a`.
        let qs = unsafe { core::slice::from_raw_parts_mut(q.as_ptr().cast::<u8>(), 64) };
        handle.raw_free(qs, crate::Alignment::from_byte_units(16), 0);
        a.reset();
    }

    #[test]
    fn retain_under_limit_splices_whole_chain() {
        let mut arena = StdArena::new();
        let p1 = talloc(&arena, 100, 8);
        let p2 = talloc(&arena, 100, 8);
        let p3 = talloc(&arena, 100, 8);
        assert_eq!(arena.allocated_bytes(), 300);
        // 300 ≤ 500: the whole live chain is spliced onto the quarantine,
        // nothing is freed, `true`.
        assert!(arena.reset_retain_with_limit(500));
        assert_eq!(arena.allocated_bytes(), 300, "retain must not reclaim");
        // New allocations start a fresh live cycle alongside the quarantine.
        let fresh = talloc(&arena, 100, 8);
        assert_eq!(arena.allocated_bytes(), 400);
        // SAFETY: fresh payload.
        unsafe { tfree(&arena, fresh, 100, 8) };
        // The next full reset frees both chains (retained + live).
        arena.reset();
        assert_eq!(arena.allocated_bytes(), 0);
        let _ = (p1, p2, p3); // freed by the reset above
    }

    #[test]
    fn retain_over_limit_frees_both_chains() {
        let mut arena = StdArena::new();
        let p1 = talloc(&arena, 100, 8);
        let p2 = talloc(&arena, 100, 8);
        assert_eq!(arena.allocated_bytes(), 200);
        // 200 > 150: all-or-nothing reclaim (mimalloc parity), `false`.
        assert!(!arena.reset_retain_with_limit(150));
        assert_eq!(arena.allocated_bytes(), 0);
        let _ = (p1, p2); // freed by the over-limit reclaim
        // Fully usable afterwards.
        let p = talloc(&arena, 64, 8);
        // SAFETY: fresh payload.
        unsafe { tfree(&arena, p, 64, 8) };
    }

    #[test]
    fn retain_fast_path_returns_true_and_keeps_live_chain() {
        let mut arena = StdArena::new();
        let p = talloc(&arena, 64, 8);
        assert!(arena.reset_retain_with_limit(1 << 20));
        assert_eq!(arena.allocated_bytes(), 64);
        // SAFETY: still live.
        unsafe { tfree(&arena, p, 64, 8) };
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn stale_handle_dealloc_of_quarantined_block() {
        let mut arena = StdArena::new();
        let other = StdArena::new();
        let p1 = talloc(&arena, 100, 16);
        let p2 = talloc(&arena, 100, 16);
        assert!(arena.reset_retain_with_limit(1 << 20)); // both quarantined
        assert_eq!(arena.allocated_bytes(), 200);
        // A caller freeing a retained block later through a handle that does
        // not own it: the unlink runs through the block's own header (now
        // threaded on the quarantine chain) and frees it — the ruling's
        // defensive layer.
        // SAFETY: `p1` is a live quarantined payload; the free path is
        // arena-agnostic.
        unsafe { tfree(&other, p1, 100, 16) };
        // The owner's counter still holds the block (documented cross-handle
        // skew; the wholesale zeroing below self-heals it).
        assert_eq!(arena.allocated_bytes(), 200);
        arena.reset(); // frees the rest of the quarantine
        assert_eq!(arena.allocated_bytes(), 0);
        let _ = p2;
    }

    #[test]
    fn grow_preserves_data_through_many_reallocations() {
        let arena = StdArena::new();
        let mut v: AllocVec<u8, &StdArena> = AllocVec::new_in(&arena);
        for i in 0..100_000u32 {
            v.push((i % 251) as u8);
        }
        assert_eq!(v.len(), 100_000);
        for (i, b) in v.iter().enumerate() {
            assert_eq!(*b, (i as u32 % 251) as u8);
        }
        drop(v);
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn allocate_zeroed_and_grow_zeroed() {
        let arena = &StdArena::new();
        let old = Layout::from_size_align(32, 16).unwrap();
        let new = Layout::from_size_align(64, 16).unwrap();
        let p = arena.allocate_zeroed(old).unwrap();
        // SAFETY: 32 zero-initialized bytes just returned.
        unsafe {
            assert!(p.as_ref().iter().all(|&b| b == 0));
        }
        let ptr = p.as_ptr().cast::<u8>();
        // SAFETY: live payload with `old` layout.
        let grown = unsafe { arena.grow_zeroed(NonNull::new(ptr).unwrap(), old, new) }.unwrap();
        // SAFETY: 64 bytes of the grown payload.
        unsafe {
            let bytes = grown.as_ref();
            assert!(bytes[..32].iter().all(|&b| b == 0), "prefix must survive grow");
            assert!(bytes[32..].iter().all(|&b| b == 0), "grow_zeroed tail must be zero");
        }
        // SAFETY: live grown payload.
        unsafe { arena.deallocate(NonNull::new(grown.as_ptr().cast::<u8>()).unwrap(), new) };
    }

    #[test]
    fn bumpalo_surface_roundtrip() {
        let arena = StdArena::new();
        let n = arena.alloc(42u64);
        assert_eq!(*n, 42);
        assert_eq!(arena.alloc_str("bao"), "bao");
        let slice = arena.alloc_slice_copy(&[1u32, 2, 3]);
        assert_eq!(slice, &[1, 2, 3]);
        let filled = arena.alloc_slice_fill_iter(0..4u8);
        assert_eq!(filled, &[0, 1, 2, 3]);
        let uninit = arena.alloc_uninit_slice::<u64>(4);
        for (i, slot) in uninit.iter_mut().enumerate() {
            slot.write(i as u64 * 7);
        }
        // SAFETY: every slot was written above.
        let init: &[u64] = unsafe { &*(uninit as *const [MaybeUninit<u64>] as *const [u64]) };
        assert_eq!(init, &[0, 7, 14, 21]);
    }

    #[test]
    fn arena_string_and_formatting() {
        let arena = StdArena::new();
        let mut s = ArenaString::new_in(&arena);
        core::fmt::Write::write_fmt(&mut s, format_args!("{}-{}", 12, "ab")).unwrap();
        s.push_str("!");
        assert_eq!(s.as_str(), "12-ab!");
        assert_eq!(s.len(), 6);
        assert!(!s.is_empty());
        let leaked: &str = s.into_bump_str();
        assert_eq!(leaked, "12-ab!");
        let from = ArenaString::from_str_in("xyz", &arena);
        assert_eq!(from.as_bytes(), b"xyz");
    }

    #[test]
    fn vec_ext_from_iter_and_leak() {
        let arena = StdArena::new();
        let v: AllocVec<i32, &StdArena> = ArenaVecExt::from_iter_in(0..5i32, &arena);
        assert_eq!(&*v, &[0, 1, 2, 3, 4]);
        assert_eq!(v.bump() as *const StdArena, &arena as *const StdArena);
        let leaked = v.into_bump_slice();
        assert_eq!(leaked, &[0, 1, 2, 3, 4]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "borrowing_default")]
    fn borrowing_default_forbids_reset() {
        let mut arena = StdArena::borrowing_default();
        arena.reset();
    }

    #[test]
    fn borrowing_default_allocates_and_drops() {
        let arena = StdArena::borrowing_default();
        let p = talloc(&arena, 32, 8);
        // SAFETY: live payload; individually freed, never bulk-freed.
        unsafe { tfree(&arena, p, 32, 8) };
        drop(arena); // Drop must not touch payloads
    }

    #[test]
    fn resize_in_place_is_false_and_remap_preserves_prefix() {
        let arena = &StdArena::new();
        let mut p = talloc(arena, 32, 8);
        assert!(!arena.resize_in_place(NonNull::new(p.as_ptr().cast::<u8>()).unwrap(), 32, 64));

        // SAFETY: live payload — write a marker, remap to 64, check prefix.
        unsafe { p.as_mut()[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]) };
        let np = arena.remap(
            NonNull::new(p.as_ptr().cast::<u8>()).unwrap(),
            32,
            64,
            8,
        );
        assert!(!np.is_null());
        // SAFETY: `np` is a live 64-byte payload with the copied prefix.
        unsafe { assert_eq!(*np.cast::<[u8; 8]>(), [1, 2, 3, 4, 5, 6, 7, 8]) };
        // SAFETY: live remapped payload.
        unsafe {
            arena.deallocate(
                NonNull::new(np).unwrap(),
                Layout::from_size_align(64, 8).unwrap(),
            )
        };
    }

    #[test]
    fn std_allocator_vtable_identity() {
        let arena = StdArena::new();
        let handle = arena.std_allocator();
        assert!(StdArena::is_instance(&handle));
        assert_eq!(StdArena::std_vtables().len(), 1);

        let p = handle
            .raw_alloc(24, crate::Alignment::from_byte_units(16), 0)
            .unwrap();
        // In-place resize is always refused on this backend.
        let slice = unsafe { core::slice::from_raw_parts_mut(p, 24) };
        assert!(!handle.raw_resize(slice, crate::Alignment::from_byte_units(16), 48, 0));
        // Live vtable allocation with the same alignment.
        handle.raw_free(slice, crate::Alignment::from_byte_units(16), 0);
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn zero_sized_layout_roundtrip() {
        let arena = StdArena::new();
        let p = talloc(&arena, 0, 1);
        // SAFETY: zero-sized payloads take the uniform small-block path.
        unsafe { tfree(&arena, p, 0, 1) };
        assert_eq!(arena.allocated_bytes(), 0);
    }

    #[test]
    fn many_allocations_free_interleaved_then_reset() {
        let mut arena = StdArena::new();
        let mut held = Vec::new();
        for i in 0..1000u32 {
            let p = talloc(&arena, (i % 64 + 1) as usize, if i % 4 == 0 { 16 } else { 8 });
            if i % 3 == 0 {
                // SAFETY: fresh payload.
                unsafe {
                    tfree(&arena, p, (i % 64 + 1) as usize, if i % 4 == 0 { 16 } else { 8 })
                };
            } else {
                held.push(p);
            }
        }
        arena.reset();
        assert_eq!(arena.allocated_bytes(), 0);
        drop(held);
    }
}

// ported from: E22 terminal design §1.2/§1.3, as refined by the 2026-09-20
// E22 ruling on this slice (2026-09-20 user verdict: remove all custom
// allocators) — stage-1 replacement backend for src/bun_alloc/MimallocArena.rs
// (entity deletion lands with S4).
