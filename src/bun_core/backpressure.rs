//! Byte-count backpressure valve — the shared decision core of the
//! response-body park/resume template.
//!
//! Two integration sites ran hand-copied versions of the same template
//! (semantic map previously lived only in commit messages 4a956b27/684ffb74):
//!
//! - **fetch streaming staging** (`bao_runtime`'s `fetch_async.rs`): staged
//!   bytes cross `UNOBSERVED_BODY_HIGH_WATER_MARK` (256 KiB) with no reader
//!   interest → park (PENDING removal + keepalive unref + transport pause);
//!   a later JS pull — reader interest, no byte test — unparks.
//! - **servo bridge channel backpressure** (`vendor/servo/components/net/
//!   fetch/bun_bridge.rs`): in-flight bytes cross `BODY_HIGH_WATER_MARK`
//!   (256 KiB) → park (drain round-trip withheld + transport pause); the
//!   consumer drops to `BODY_LOW_WATER_MARK` (128 KiB) → resume (hysteresis
//!   against per-frame park/resume ping-pong).
//!
//! The transport levers themselves (uSockets read pause, h2 stream-window
//! withholding, QUIC `want_read(false)`, drain round-trips) stay at each
//! site: this valve says **when**, never **how**.
//!
//! Transport-side pause *gates* (`bun_http`'s `response_transport_paused`,
//! the h3 client's `Stream::transport_paused`) are deliberately NOT
//! integration sites — they are the commanded receiving end of the pause
//! queue, with no byte metering and no watermark decision to share.
//!
//! ## Memory-ordering contract
//!
//! The latch protocol matches the orderings the two sites already used, so
//! adopting the valve is a zero-behavior-change refactor:
//!
//! - park latch write: `Release` (the site's transport pause is scheduled
//!   after the latch — the consumer's `is_parked` read must not miss it);
//! - park-step read / drain-side read: `Acquire`;
//! - resume arms: `swap(false, AcqRel)` — the take is the synchronization
//!   point that orders the site's transport resume + re-issued drain.
//!
//! ## Consistency discipline for lock-guarded metering
//!
//! The bridge updates `level` cross-thread (HTTPThread producer ⇄ tokio
//! consumer) through the atomics directly. The fetch site embeds the valve
//! next to a `Mutex`-guarded staging deque and performs every `level` update
//! **while holding that lock** — the level and the deque it measures stay in
//! the same critical section, exactly as the plain `usize` it replaced.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Producer-side outcome of one park-step evaluation — the three arms every
/// copy of the template had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkStep {
    /// Level reached the high-water mark and the valve just latched parked.
    /// The caller pauses its transport read side NOW and withholds any
    /// drain/refill round-trip — withholding it IS the park.
    Parked,
    /// Below the mark and not parked: flow. The caller round-trips its
    /// drain/refill so the transport keeps reading.
    Flowing,
    /// Already parked (an earlier delivery latched it). The caller does
    /// neither — the consumer's resume arm re-arms the transport and
    /// re-issues the withheld drain.
    Held,
}

/// Byte-level watermarks + metering + the park latch — the shared core of
/// the response-body backpressure template (see the [module
/// docs](self) for the two integration shapes).
///
/// `level` counts producer-forwarded-but-unconsumed bytes; the producer arm
/// parks at `high`, the consumer arm resumes — byte-hysteresis at `low`, or
/// interest-driven with no byte test, depending on the site.
#[derive(Debug)]
pub struct BackpressureValve {
    /// Park threshold: producer parks once `level` reaches this.
    high: usize,
    /// Byte-hysteresis resume threshold: the consumer's
    /// [`resume_below_low`](Self::resume_below_low) arm fires once `level`
    /// drops to this. Interest-driven sites never call that arm.
    low: usize,
    /// Bytes forwarded but not yet consumed.
    level: AtomicUsize,
    /// Parked latch. See the module ordering contract.
    parked: AtomicBool,
}

impl BackpressureValve {
    /// Full hysteresis valve (the bridge shape): park at/above `high`,
    /// byte-resume at/below `low` (`low < high` keeps a dead band so
    /// per-frame park/resume ping-pong cannot starve the flow).
    pub const fn with_hysteresis(high: usize, low: usize) -> Self {
        BackpressureValve {
            high,
            low,
            level: AtomicUsize::new(0),
            parked: AtomicBool::new(false),
        }
    }

    /// Single-mark valve (the fetch staging shape): park at/above `high`;
    /// resume is interest-driven (the consumer pulls —
    /// [`take_parked`](Self::take_parked)), so the byte-resume arm is never
    /// consulted and `low` pins to `high` (a byte resume would require a
    /// full drain — the conservative identity for an unread arm).
    pub const fn park_at(high: usize) -> Self {
        BackpressureValve {
            high,
            low: high,
            level: AtomicUsize::new(0),
            parked: AtomicBool::new(false),
        }
    }

    /// Producer metering: account `n` newly forwarded bytes. Returns the
    /// post-add level — exactly the value the park decision reads.
    /// `n == 0` is a pure level read (the empty-delivery probe).
    pub fn note_produced(&self, n: usize) -> usize {
        self.level.fetch_add(n, Ordering::AcqRel) + n
    }

    /// Consumer metering: retire `n` consumed bytes. Returns the post-sub
    /// level (saturating — accounting must never underflow). Fetch sites
    /// call this while holding the lock that guards the staging deque the
    /// level measures.
    pub fn note_consumed(&self, n: usize) -> usize {
        self.level.fetch_sub(n, Ordering::AcqRel).saturating_sub(n)
    }

    /// Current level (diagnostics + snapshot park checks).
    pub fn level(&self) -> usize {
        self.level.load(Ordering::Acquire)
    }

    /// Park-latch state (the producer's drain-side read: parked deliveries
    /// withhold the drain round-trip).
    pub fn is_parked(&self) -> bool {
        self.parked.load(Ordering::Acquire)
    }

    /// Producer park step: ONE latch read, three arms (see [`ParkStep`]).
    /// The load-then-store shape is deliberate — it preserves the
    /// interleaving semantics of the original templates (a concurrent
    /// consumer resume is not raced out by a compare-exchange).
    pub fn producer_park_step(&self, level: usize) -> ParkStep {
        if self.is_parked() {
            ParkStep::Held
        } else if level >= self.high {
            self.parked.store(true, Ordering::Release);
            ParkStep::Parked
        } else {
            ParkStep::Flowing
        }
    }

    /// Consumer byte-hysteresis resume: once `level` drops to/below the
    /// low-water mark, take the latch. Returns `true` exactly once per park
    /// — the caller then resumes the transport and re-issues the withheld
    /// drain round-trip. Short-circuits on the byte test first (an
    /// unparked valve never pays the swap).
    pub fn resume_below_low(&self, level: usize) -> bool {
        level <= self.low && self.parked.swap(false, Ordering::AcqRel)
    }

    /// Interest-driven resume (no byte test): the consumer showed up, take
    /// the latch. Returns `true` exactly once per park.
    pub fn take_parked(&self) -> bool {
        self.parked.swap(false, Ordering::AcqRel)
    }

    /// Latch parked unconditionally — for compound park checks that
    /// evaluate site-specific conditions (reader interest, phase) before
    /// committing; the fetch staging park check is the canonical user.
    pub fn latch_park(&self) {
        self.parked.store(true, Ordering::Release);
    }

    /// Pure predicate: `level` at/above the high-water mark (compound park
    /// checks mix this with their site-specific terms).
    pub fn above_high(&self, level: usize) -> bool {
        level >= self.high
    }
}
