//! Thread-local arena allocator for AST-interior `Vec`s.
//!
//! Strategy B for the require-cache ESM leak (docs/BABYLIST_REPLACEMENT.md):
//! `G::DeclList` / `G::PropertyList` / `ExprNodeList` / `ClassStaticBlock::stmts`
//! were ported from Zig `BabyList<T>` to global-heap `Vec<T>`. The AST *nodes*
//! that embed those `Vec` headers live in `ASTMemoryAllocator`'s `MimallocArena`
//! and are bulk-freed (no `Drop`) on `enter()` → `arena.reset()`, so the global
//! buffers leak — one full AST's worth of `Vec` backing storage per imported
//! module in `RuntimeTranspilerStore`.
//!
//! `AstAlloc` is a ZST `core::alloc::Allocator` that routes `allocate`/`grow`
//! to the thread's active [`AstAllocState`] (installed by
//! `ASTMemoryAllocator::push`/`Scope::enter` and friends), and makes
//! `deallocate` a **no-op**. Everything allocated through a state is bulk-freed
//! when its owner resets or releases it. When no state is installed the
//! allocator falls back to the raw default heap (plain libc via
//! `fallback::C_ALLOCATOR`), matching the pre-Strategy-B behaviour for the
//! bundler / `Stmt.Data.Store` block-store path.
//!
//! `deallocate` being a no-op preserves the `Expr::Data::clone_in` invariant
//! (`src/js_parser/ast/Expr.rs:2178`): payloads are `core::ptr::read`-copied
//! under the assumption "no `Drop`, no owned heap state". Two `Vec<_, AstAlloc>`
//! headers may therefore alias the same buffer — neither ever has `Drop` run
//! (both live in arena slots), but if one *did*, the no-op `deallocate` keeps
//! the bitwise copy sound.
//!
//! Placed in `bun_alloc` (not `js_parser`) so that `bun_ast::
//! ExprNodeList` and `bun_collections::VecExt` — both below `js_parser` in the
//! crate graph — can name `Vec<T, AstAlloc>`.

use crate::core_alloc::{AllocError, Allocator, Layout};
use core::cell::Cell;
use core::mem::MaybeUninit;
use core::ptr::NonNull;

use crate::{alloc_result, Arena};
use crate::fallback::C_ALLOCATOR;

// The parser builds thousands of tiny `AstVec`s; allocations `<= BUMP_MAX` are
// carved from a 16 KB buffer stored inline in the state (mirroring Zig's
// `StackFallbackAllocator`), so the small case never reaches the heap. The
// cursor, the buffer, and the spill target live in one struct, so none of them
// can outlive the others.

/// Largest allocation served from the inline bump chunk; above this, requests
/// go straight to the spill heap.
const BUMP_MAX: usize = 512;

/// Inline bump chunk size. No refill — once full, small allocations fall
/// through to the spill heap.
const BUMP_CHUNK: usize = 16 * 1024;

/// Per-scope allocation state for [`AstAlloc`]. Owned by whichever component
/// opened the AST allocation scope and moved into the [`AST_ALLOC`]
/// thread-local while the scope is active; the owner decides when the
/// contents are bulk-freed.
pub struct AstAllocState {
    /// Offset of the next free byte in `bump_chunk`.
    bump_cursor: usize,
    /// Spill target for allocations the chunk can't serve. Set by the
    /// installing scope from its own arena ([`Self::set_spill_heap`]); the
    /// installer guarantees the arena outlives the installed window. Null when
    /// the installer has no arena — `owned_spill` is then created lazily.
    spill: *const Arena,
    /// Backing storage for `spill` when no borrowed target was provided.
    owned_spill: Option<Arena>,
    /// Inline small-allocation buffer.
    bump_chunk: [MaybeUninit<u8>; BUMP_CHUNK],
}

impl AstAllocState {
    /// Allocate a clean state without materialising 16 KB on the stack.
    fn new_boxed() -> Box<Self> {
        let mut boxed = Box::<Self>::new_uninit();
        let p = boxed.as_mut_ptr();
        // SAFETY: the header fields are written before `assume_init`;
        // `bump_chunk` is `MaybeUninit` and may stay uninitialised.
        unsafe {
            (&raw mut (*p).bump_cursor).write(0);
            (&raw mut (*p).spill).write(core::ptr::null_mut());
            (&raw mut (*p).owned_spill).write(None);
            boxed.assume_init()
        }
    }

    /// Bulk-free everything allocated through this state. Any pointer
    /// previously returned by [`AstAlloc`] under this state is invalidated.
    #[inline]
    pub fn reset(&mut self) {
        self.bump_cursor = 0;
        self.spill = core::ptr::null_mut();
        self.owned_spill = None;
    }

    /// Point spill allocations at `arena` (the installing scope's arena), which
    /// must outlive the installed window. Called on every install so an arena
    /// reset between installs is picked up.
    #[inline]
    pub fn set_spill_heap(&mut self, arena: *const Arena) {
        debug_assert!(
            self.owned_spill.is_none(),
            "AstAllocState: switching an owned spill heap to a borrowed one would strand its contents"
        );
        self.spill = arena;
    }

    /// Carve `size` bytes at `align` (a power of two)
    /// from the inline chunk. `None` when it doesn't fit — there is no refill;
    /// the caller falls through to the spill heap.
    #[inline]
    fn bump_alloc(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        debug_assert!(size != 0 && size <= BUMP_MAX && align.is_power_of_two());
        debug_assert!(self.bump_cursor <= BUMP_CHUNK);
        // SAFETY: `bump_cursor <= BUMP_CHUNK` (invariant: only advanced below
        // after the bounds check), so `add` is at most one-past-the-end.
        let cur = unsafe {
            self.bump_chunk
                .as_mut_ptr()
                .cast::<u8>()
                .add(self.bump_cursor)
        };
        let remaining = BUMP_CHUNK - self.bump_cursor;
        let pad = cur.align_offset(align);
        if pad <= remaining && size <= remaining - pad {
            // SAFETY: `pad + size <= remaining`, so `cur + pad` and
            // `cur + pad + size` stay within `bump_chunk` (one-past-the-end at
            // most).
            unsafe {
                let aligned = cur.add(pad);
                self.bump_cursor += pad + size;
                Some(aligned)
            }
        } else {
            None
        }
    }

    /// The state's spill arena: the borrowed target installed by
    /// [`Self::set_spill_heap`], or a lazily created owned arena when none was
    /// provided. Allocation only ever takes `&Arena`, so the handle is a
    /// `*const`.
    #[inline]
    fn spill_handle(&mut self) -> *const Arena {
        if !self.spill.is_null() {
            return self.spill;
        }
        // The state lives in a `Box` inside the thread-local, so `owned_spill`'s
        // address is stable for as long as this handle can be used (the same
        // stability invariant the mi_heap handle relied on).
        let owned = self.owned_spill.insert(Arena::new()) as *mut Arena;
        self.spill = owned;
        self.spill
    }
}

// ── Thread-local active state ────────────────────────────────────────────────

/// The active [`AstAllocState`], or `None` when no AST scope is installed
/// (allocations then fall back to the raw default heap).
///
/// `#[thread_local]` (not `thread_local!`): read on every `AstAlloc`
/// allocation, so it must stay a bare `__thread` slot. Stable (no
/// `#[thread_local]` attribute) uses a const-init `thread_local!` — no
/// lazy-init branch, just the `LocalKey::with` frame on the hot path.
///
/// BCE (TLS-destructor vs "no destructor" claim, 2026-08-20 heap-hunt #1,
/// same class as ARENA_POOL in ast_memory_allocator.rs): const-init does
/// NOT remove the destructor — the stable arm really dropped an active
/// `Box<AstAllocState>` at thread exit (the nightly bare slot never does),
/// a channel divergence plus an unspecified ordering window against
/// mimalloc's own thread-teardown. The stable arm now severs the dtor
/// chain with `ManuallyDrop` (accessors hand the live box across the seam
/// exactly once); an abandoned active box at thread exit is reclaimed by
/// the process exit / mimalloc teardown, matching nightly semantics.
#[cfg(bao_nightly)]
#[thread_local]
static AST_ALLOC: Cell<Option<Box<AstAllocState>>> = Cell::new(None);

#[cfg(not(bao_nightly))]
std::thread_local! {
    static AST_ALLOC: Cell<Option<::core::mem::ManuallyDrop<Box<AstAllocState>>>> =
        const { Cell::new(None) };
}

// One-slot recycler so a per-job `acquire_state`/`release_state` pair doesn't
// pay a 16 KB malloc each time. Uses `thread_local!` (unlike `AST_ALLOC`) so
// the destructor frees a parked box at thread exit; only touched on scope
// entry/exit, never on the allocation hot path.
std::thread_local! {
    static AST_ALLOC_SPARE: Cell<Option<Box<AstAllocState>>> = const { Cell::new(None) };
}

/// Run `f` against the installed state (if any) without moving the box out
/// of the thread-local. Dual-mode: bare-slot pointer read on nightly; a
/// const-initialized `thread_local!` + `with` on stable (same no-lazy-init /
/// no-dtor storage class, one extra call frame that inlines away).
#[inline(always)]
fn with_active_state<R>(f: impl FnOnce(Option<&mut AstAllocState>) -> R) -> R {
    #[cfg(bao_nightly)]
    {
        // SAFETY: `AST_ALLOC` is thread-local and this module never re-enters
        // itself while the reference is live, so this is the only reference
        // to the boxed state for its lifetime.
        unsafe { f((*AST_ALLOC.as_ptr()).as_deref_mut()) }
    }
    #[cfg(not(bao_nightly))]
    {
        AST_ALLOC.with(|slot| {
            // SAFETY: const-initialized `thread_local!` storage lives for the
            // thread's lifetime; the same never-re-enters-itself argument as
            // the nightly side holds for the reference's lifetime. The
            // ManuallyDrop seam is transparent here: a Some slot always wraps
            // a live, uniquely-owned box.
            f(unsafe {
                // Explicit-typed intermediate binding — Deref coercion
                // (ManuallyDrop<Box<T>> -> &mut Box<T>) resolves at the
                // annotation, identically on both channels (stable E0308
                // fix, run6; ManuallyDrop::deref_mut itself is
                // nightly-only and must not be named here).
                (*slot.as_ptr()).as_mut().map(|md| {
                    let b: &mut Box<AstAllocState> = md;
                    &mut **b
                })
            })
        })
    }
}

/// Take the recycled spare state for this thread, or allocate a fresh one.
#[inline]
pub fn acquire_state() -> Box<AstAllocState> {
    AST_ALLOC_SPARE
        .try_with(Cell::take)
        .ok()
        .flatten()
        .unwrap_or_else(AstAllocState::new_boxed)
}

/// Bulk-free `state`'s allocations and park the clean box in the recycler.
/// If the slot is occupied (or the thread is tearing down) the box is freed.
#[inline]
pub fn release_state(mut state: Box<AstAllocState>) {
    state.reset();
    drop(AST_ALLOC_SPARE.try_with(|slot| slot.replace(Some(state))));
}

/// Replace the active allocation state, returning the previous occupant. The
/// caller passes the previous occupant back when its scope exits; `None`
/// detaches to the raw-default-heap fallback.
#[inline]
pub fn swap_state(state: Option<Box<AstAllocState>>) -> Option<Box<AstAllocState>> {
    #[cfg(bao_nightly)]
    {
        AST_ALLOC.replace(state)
    }
    #[cfg(not(bao_nightly))]
    {
        // SAFETY: both directions hand a live, uniquely-owned box across the
        // ManuallyDrop seam exactly once (mirror of the ARENA_POOL fix).
        AST_ALLOC
            .with(|slot| slot.replace(state.map(::core::mem::ManuallyDrop::new)))
            .map(|mut md| unsafe { ::core::mem::ManuallyDrop::take(&mut md) })
    }
}

/// Address of the active state (null when none is installed). Identity checks
/// only; never dereferenced.
#[inline]
pub fn active_state_id() -> *const AstAllocState {
    #[cfg(bao_nightly)]
    {
        // SAFETY: see `with_active_state` — shared read of the thread-local
        // slot; identity checks only, never dereferenced.
        unsafe { (*AST_ALLOC.as_ptr()).as_deref() }.map_or(core::ptr::null(), core::ptr::from_ref)
    }
    #[cfg(not(bao_nightly))]
    {
        AST_ALLOC.with(|slot| {
            // SAFETY: const-init thread-local storage; identity only. The
            // ManuallyDrop seam derefs to the same Box address.
            unsafe {
                // Same explicit-typed intermediate as with_active_state.
                (*slot.as_ptr()).as_ref().map(|md| {
                    let b: &Box<AstAllocState> = md;
                    &**b
                })
            }
                .map_or(core::ptr::null(), core::ptr::from_ref)
        })
    }
}

/// Bulk-free the *installed* state in place. For owners that keep their state
/// installed across resets and so cannot reach the box through their own
/// field. No-op when no state is installed.
#[inline]
pub fn reset_active_state() {
    with_active_state(|state| {
        if let Some(state) = state {
            state.reset();
        }
    });
}

/// [`AstAllocState::set_spill_heap`] on the *installed* state. No-op when no
/// state is installed.
#[inline]
pub fn set_active_spill_heap(arena: *const Arena) {
    with_active_state(|state| {
        if let Some(state) = state {
            state.set_spill_heap(arena);
        }
    });
}

/// RAII guard: for its lifetime, [`AstAlloc`] allocates on the **default heap**
/// instead of the active per-parse state. Use when constructing
/// `AstVec`/`StoreRef` data that must outlive the current parse arena
/// (e.g. `Expr::deep_clone` for `WorkspacePackageJSONCache`). Without this,
/// the next `ASTMemoryAllocator::reset()` frees buffers the cache still holds.
///
/// Restores the prior state on drop, so it nests correctly inside an
/// `ASTMemoryAllocator` scope.
pub struct DetachAstHeap(Option<Box<AstAllocState>>);
impl DetachAstHeap {
    #[inline]
    pub fn new() -> Self {
        Self(swap_state(None))
    }
}
impl Drop for DetachAstHeap {
    #[inline]
    fn drop(&mut self) {
        let displaced = swap_state(self.0.take());
        debug_assert!(
            displaced.is_none(),
            "AstAlloc scope installed during a DetachAstHeap window was not uninstalled"
        );
    }
}

/// RAII scope that installs a fresh (or recycled) [`AstAllocState`] for its
/// lifetime and bulk-frees everything allocated through it on drop. For
/// callers that want arena-lifetime `AstVec`s without an `ASTMemoryAllocator`.
pub struct ScopedAstAlloc {
    prev: Option<Box<AstAllocState>>,
}
impl ScopedAstAlloc {
    /// Install a state whose spill allocations land in `spill_arena`, which
    /// must stay live (and not be reset) for the guard's entire lifetime.
    #[inline]
    pub fn with_spill(spill_arena: *const Arena) -> Self {
        let mut state = acquire_state();
        state.set_spill_heap(spill_arena);
        Self {
            prev: swap_state(Some(state)),
        }
    }

    /// Install a state with a lazily created owned spill heap.
    #[inline]
    pub fn new() -> Self {
        Self {
            prev: swap_state(Some(acquire_state())),
        }
    }

    /// Uninstall the scope's state and return it **without** bulk-freeing it,
    /// restoring the previous occupant exactly as `drop` would. For callers
    /// that hand the parsed AST to an async consumer: small `AstVec`s live in
    /// the state's inline chunk, so the returned box must be kept alive until
    /// the consumer is done with the AST.
    #[inline]
    pub fn take_state(self) -> Option<Box<AstAllocState>> {
        let mut this = core::mem::ManuallyDrop::new(self);
        let installed = swap_state(this.prev.take());
        debug_assert!(
            installed.is_some(),
            "ScopedAstAlloc state was uninstalled by someone else"
        );
        installed
    }
}
impl Default for ScopedAstAlloc {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for ScopedAstAlloc {
    #[inline]
    fn drop(&mut self) {
        match swap_state(self.prev.take()) {
            Some(state) => release_state(state),
            None => debug_assert!(
                false,
                "ScopedAstAlloc state was uninstalled by someone else"
            ),
        }
    }
}

/// Zero-sized `Allocator` that routes to the active [`AstAllocState`] when one
/// is installed, else to the raw default heap. `deallocate` is a no-op (the state's
/// owner reclaims everything in bulk).
///
/// Use as `Vec<T, AstAlloc>` (see [`AstVec`]). The ZST means the `Vec` stays
/// 24 bytes — same size as `Vec<T>` — so AST node layouts are unchanged.
#[derive(Clone, Copy, Default)]
pub struct AstAlloc;

/// `Vec` whose backing buffer lives in the thread-local AST allocation state.
#[cfg(bao_nightly)]
pub type AstVec<T> = alloc::vec::Vec<T, AstAlloc>;
#[cfg(not(bao_nightly))]
pub type AstVec<T> = crate::core_alloc::AllocVec<T, AstAlloc>;

/// `Box` in the same arena ([`AstVec`]'s boxed-slice sibling — the
/// `Box<[u8], AstAlloc>` shape key-records and export-alias lists use).
#[cfg(bao_nightly)]
pub type AstBox<T> = alloc::boxed::Box<T, AstAlloc>;
#[cfg(not(bao_nightly))]
pub type AstBox<T> = crate::core_alloc::AllocBox<T, AstAlloc>;

#[inline(always)]
fn heap_alloc(layout: Layout) -> *mut u8 {
    with_active_state(|state| match state {
        // Global fallback (no AST scope active): the raw default heap. Plain
        // libc `malloc` tolerates `size == 0` (unique non-null pointer), so no
        // special-casing.
        None => C_ALLOCATOR
            .raw_alloc(
                layout.size(),
                crate::Alignment::from_byte_units(layout.align()),
                0,
            )
            .unwrap_or_else(core::ptr::null_mut),
        Some(state) => {
            // Small requests: carve from the state's inline chunk so a burst
            // of tiny `AstVec`s costs zero mallocs. Zero-size layouts fall
            // through (a `Layout` of size 0 is always validly served by the
            // arena's `alloc_layout`).
            if layout.size() != 0 && layout.size() <= BUMP_MAX {
                if let Some(p) = state.bump_alloc(layout.size(), layout.align()) {
                    return p;
                }
            }
            // SAFETY: `spill_handle` returns the live spill arena owned by
            // `state`, which the thread-local keeps alive for the duration of
            // this call; allocation takes the arena by shared reference.
            unsafe { &*state.spill_handle() }.alloc_layout(layout).as_ptr()
        }
    })
}

// SAFETY:
// - `allocate`/`grow` return blocks carved from the active state's inline
//   chunk, from the state's spill arena, or from the raw default heap when no
//   state is installed; all satisfy `layout`. State-owned blocks are bulk-freed
//   when the owner resets/releases the state.
// - `deallocate` is a no-op (permitted: the trait only requires that memory
//   *may* be reclaimed). This preserves the `Expr::Data::clone_in` invariant
//   (two `Vec` headers may alias one buffer; neither frees it). Under the
//   global fallback the buffer leaks until process exit — the documented
//   pre-Strategy-B status quo.
// - `grow` always allocates a fresh block + `memcpy` + abandons the old one.
//   No in-place resize exists on this backend: a block smaller than `BUMP_MAX`
//   may be a bump-chunk interior pointer (see `heap_alloc`), and even a larger
//   block cannot be proven to be this state's spill-arena payload — a `Vec`
//   clone can cross threads (`BundleV2::clone_ast` does exactly this), so the
//   pointer may be a raw default-heap block from a since-exited scope, which
//   must not be handed to any arena free path. The old block is abandoned
//   (same leak semantics as `deallocate` — and under a state it, like every
//   other block, is reclaimed when the owner resets the state).
// - `allocate_zeroed` returns zero-filled memory (the spill arena's zeroed
//   allocation, or an explicit zero fill on the raw heap); same lifetime as
//   `allocate`.
// - `AstAlloc` is a ZST: every instance is trivially "the same allocator", so
//   the "pointers may be freed by any clone" requirement is satisfied.
// - `Send + Sync` (auto-derived for a fieldless ZST) is sound: each call reads
//   the *calling* thread's `AST_ALLOC`, and allocation is gated to that thread
//   by `ASTMemoryAllocator`'s single-threaded contract (mirrored from Zig's
//   `ThreadLock`; see `MimallocArena::assert_owning_thread`). The no-op
//   `deallocate` removes the only cross-thread hazard a `Vec<_,A>: Send` would
//   otherwise introduce.
unsafe impl Allocator for AstAlloc {
    #[inline]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        alloc_result(heap_alloc(layout), layout.size())
    }

    #[inline]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Zero-filled on both arms (the spill arena's zeroed allocation, or an
        // explicit fill over the raw-heap block). Never bump-carved (the chunk
        // is uninitialised); same lifetime semantics as `heap_alloc`.
        let p: *mut u8 = with_active_state(|state| match state {
            None => {
                let p = C_ALLOCATOR
                    .raw_alloc(
                        layout.size(),
                        crate::Alignment::from_byte_units(layout.align()),
                        0,
                    )
                    .unwrap_or_else(core::ptr::null_mut);
                if !p.is_null() {
                    // SAFETY: `p` is `layout.size()` writable bytes just
                    // returned above.
                    unsafe { core::ptr::write_bytes(p, 0, layout.size()) };
                }
                p
            }
            // SAFETY: `spill_handle` returns the live spill arena owned by the
            // installed state; see `heap_alloc`.
            Some(state) => match unsafe { &*state.spill_handle() }.allocate_zeroed(layout) {
                Ok(p) => p.as_ptr().cast::<u8>(),
                Err(_) => core::ptr::null_mut(),
            },
        });
        alloc_result(p, layout.size())
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Unconditional no-op — see SAFETY block above and the module doc's
        // `Expr::Data::clone_in` invariant. Under an installed state the block
        // is reclaimed when the owner resets/releases the state; under the
        // global fallback it leaks (cannot prove `ptr`'s provenance).
        let _ = (ptr, layout);
    }

    #[inline]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Always the relocate path (see the SAFETY block above: no in-place
        // resize exists on this backend, and a live pointer cannot be proven
        // to belong to this state's spill arena).
        let p = NonNull::new(heap_alloc(new)).ok_or(AllocError)?;
        // SAFETY: `p` is a fresh `new.size()`-byte block disjoint from `ptr`;
        // `old.size()` bytes at `ptr` are initialized per the `grow` contract;
        // `old.size() <= new.size()`.
        unsafe { core::ptr::copy_nonoverlapping(ptr.as_ptr(), p.as_ptr(), old.size()) };
        Ok(NonNull::slice_from_raw_parts(p, new.size()))
    }

    #[inline]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Keep the existing slot — it already holds ≥ `new.size()` bytes at ≥
        // `old.align()` alignment, and `new.size() <= old.size()` per the
        // `Allocator::shrink` contract. No `mi_realloc`: see `grow` note.
        debug_assert!(new.align() <= old.align());
        let _ = old;
        Ok(NonNull::slice_from_raw_parts(ptr, new.size()))
    }
}

// ── AstVec construction helpers ──────────────────────────────────────────
// `Vec<T, A>` has no `Default` / `From<&[T]>` for non-`Global` `A`, so the
// 81 `DeclList::default()` / `::from_slice()` etc. call sites need these.
// Kept as free fns (not a trait) so `bun_collections::VecExt` can add a
// blanket `impl<T> VecExt<T> for Vec<T, AstAlloc>` that forwards here without
// a `bun_alloc → bun_collections` cycle.

impl AstAlloc {
    /// `Vec::new()` parity. `const` so it is usable in `Default` impls.
    #[inline]
    pub const fn vec<T>() -> AstVec<T> {
        crate::core_alloc::AllocVec::new_in(AstAlloc)
    }

    /// `Vec::with_capacity` parity.
    #[inline]
    pub fn vec_with_capacity<T>(cap: usize) -> AstVec<T> {
        crate::core_alloc::AllocVec::with_capacity_in(cap, AstAlloc)
    }

    /// `<[T]>::to_vec` parity (Zig: `BabyList.fromSlice`).
    #[inline]
    pub fn vec_from_slice<T: Clone>(items: &[T]) -> AstVec<T> {
        let mut v = crate::core_alloc::AllocVec::with_capacity_in(items.len(), AstAlloc);
        v.extend_from_slice(items);
        v
    }

    /// Move `items` element-wise into a fresh AST-heap allocation. Replaces
    /// both `VecExt::from_owned_slice` (`Box<[T]>` → `Vec`) and
    /// `VecExt::from_bump_slice` (leaked `&mut [T]` → `Vec`): in either case
    /// the source storage is on the wrong heap, so a copy is unavoidable.
    #[inline]
    pub fn vec_from_iter<T, I: IntoIterator<Item = T>>(iter: I) -> AstVec<T> {
        let iter = iter.into_iter();
        let (lo, _) = iter.size_hint();
        let mut v = crate::core_alloc::AllocVec::with_capacity_in(lo, AstAlloc);
        v.extend(iter);
        v
    }
}

// NOTE: `impl<T> Default for Vec<T, AstAlloc>` is rejected by orphan rules
// (`T` is an uncovered type param appearing before the local `AstAlloc` in
// `Vec`'s parameter list). `core::mem::take` therefore cannot be used on
// `AstVec<T>`; call [`AstAlloc::take`] instead.
impl AstAlloc {
    /// `core::mem::take` for [`AstVec`] (whose `Default` impl is blocked by
    /// orphan rules). Replaces `*v` with an empty vec and returns the old
    /// contents.
    #[inline]
    pub fn take<T>(v: &mut AstVec<T>) -> AstVec<T> {
        core::mem::replace(v, crate::core_alloc::AllocVec::new_in(AstAlloc))
    }
}
