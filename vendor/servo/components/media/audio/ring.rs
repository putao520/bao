/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A bounded, lock-free single-producer/single-consumer ring buffer.
//!
//! This is the only thread primitive added by the AudioWorklet bridge: it is
//! built from `std::sync::atomic` alone (no allocation on push or pop, no
//! locks, no syscalls), so it is safe to use from the audio render thread's
//! block-processing path.
//!
//! Slots are preallocated at construction time; `push` and `pop` move values
//! in and out of preallocated slots and never allocate.
//!
//! Two policies, both non-blocking:
//!
//! * *Backpressure*: `push` fails with [`SpscRingError::Full`] when the ring
//!   holds `capacity` values. The producer decides what to do (drop, retry
//!   later, or propagate the error as flow control).
//! * *Underrun*: `pop` returns `None` when the ring is empty.
//!
//! Port messages (`MessagePort.postMessage` between the main thread and the
//! AudioWorkletGlobalScope) use the same primitive through the
//! [`PortMessageRing`](crate::audioworklet_node::PortMessageRing) alias, where
//! `push` failures surface as explicit backpressure to the sender instead of
//! silently growing an unbounded queue.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Error returned by [`SpscRing::push`].
#[derive(Debug, PartialEq)]
pub enum SpscRingError<T> {
    /// The ring is full; the value is returned to the producer.
    Full(T),
}

/// A bounded, lock-free SPSC ring buffer with preallocated slots.
///
/// * Producer thread: [`push`](Self::push) only.
/// * Consumer thread: [`pop`](Self::pop) only.
/// * Both sides may call the observation helpers (`len`, counters).
///
/// The capacity is rounded up to a power of two so that slot indexing is a
/// mask on the monotonic counters. Both counters are allowed to wrap: only
/// their difference is meaningful, and `capacity <= u64::MAX / 2` keeps the
/// difference well-defined for any realistic lifetime.
pub struct SpscRing<T> {
    /// Preallocated slots. Slot `i & self.mask` is owned exclusively by the
    /// producer while it holds the push-side token `tail == i`, and
    /// exclusively by the consumer while it holds `head == i`; the two token
    /// ranges `[tail, tail + capacity)` and `[head, tail)` are disjoint by
    /// the `tail - head <= capacity` invariant, so no slot is ever accessed
    /// concurrently.
    slots: Box<[UnsafeCell<Option<T>>]>,
    mask: u64,
    /// Position of the next value to read. Written only by the consumer.
    head: AtomicU64,
    /// Position of the next value to write. Written only by the producer.
    tail: AtomicU64,
    /// Number of values dropped by `push` because the ring was full.
    dropped: AtomicU64,
}

// SAFETY: the ring transfers `T` values from the producer thread to the
// consumer thread through the Release/Acquire pair on `tail` (push) and
// `head` (pop). A `T: Send` value is safe to move across threads.
unsafe impl<T: Send> Sync for SpscRing<T> {}
unsafe impl<T: Send> Send for SpscRing<T> {}

impl<T> SpscRing<T> {
    /// Create a ring holding at least `capacity` values.
    ///
    /// The capacity is rounded up to a power of two. All slots start empty;
    /// nothing is allocated after this call returns.
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1).next_power_of_two();
        let slots = (0..capacity)
            .map(|_| UnsafeCell::new(None))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            mask: (capacity - 1) as u64,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Maximum number of in-flight values.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Enqueue a value, or return it in [`SpscRingError::Full`] when the ring
    /// holds `capacity` values. Wait-free: a single atomic store on the
    /// producer's counter, no allocation, no lock.
    pub fn push(&self, value: T) -> Result<(), SpscRingError<T>> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail.wrapping_sub(self.head.load(Ordering::Acquire)) >= self.slots.len() as u64 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(SpscRingError::Full(value));
        }
        // SAFETY: `tail - head < capacity`, so slot `tail & mask` was last
        // written `capacity` pushes ago and that value was already taken by
        // the consumer (`head > tail - capacity`). No other thread touches
        // this slot until we publish the new `tail`.
        unsafe {
            *self.slots[(tail & self.mask) as usize].get() = Some(value);
        }
        self.tail.store(tail + 1, Ordering::Release);
        Ok(())
    }

    /// Dequeue the oldest value, or `None` when the ring is empty. Wait-free:
    /// a single atomic store on the consumer's counter, no allocation, no
    /// lock.
    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: `head != tail`, so slot `head & mask` holds a value that the
        // producer published (Release on `tail`) and is visible to us (Acquire
        // on `tail`). No other thread touches this slot until we publish the
        // new `head`.
        let value = unsafe { (*self.slots[(head & self.mask) as usize].get()).take() };
        debug_assert!(value.is_some(), "slot was published by the producer");
        self.head.store(head + 1, Ordering::Release);
        value
    }

    /// Number of values currently in flight.
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head) as usize
    }

    /// Whether the ring currently holds no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total number of values dropped by `push` because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn fifo_order_and_counters() {
        let ring: SpscRing<u32> = SpscRing::with_capacity(4);
        assert!(ring.is_empty());
        assert_eq!(ring.capacity(), 4);
        for i in 0..4u32 {
            assert_eq!(ring.push(i), Ok(()));
        }
        assert_eq!(ring.len(), 4);
        // Full: the value comes back to the producer.
        assert_eq!(ring.push(100), Err(SpscRingError::Full(100)));
        assert_eq!(ring.dropped(), 1);
        for i in 0..4u32 {
            assert_eq!(ring.pop(), Some(i));
        }
        // Underrun.
        assert_eq!(ring.pop(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn capacity_rounds_up_to_power_of_two() {
        let ring: SpscRing<u8> = SpscRing::with_capacity(5);
        assert_eq!(ring.capacity(), 8);
        let ring: SpscRing<u8> = SpscRing::with_capacity(0);
        assert_eq!(ring.capacity(), 1);
        assert_eq!(ring.push(1), Ok(()));
        assert_eq!(ring.push(2), Err(SpscRingError::Full(2)));
        assert_eq!(ring.pop(), Some(1));
    }

    #[test]
    fn spsc_thread_handoff_preserves_order() {
        const COUNT: u64 = 20_000;
        const CAPACITY: usize = 1024;
        let ring = Arc::new(SpscRing::<u64>::with_capacity(CAPACITY));
        let producer = Arc::clone(&ring);
        let consumer = Arc::clone(&ring);
        let handle = thread::spawn(move || {
            for i in 0..COUNT {
                // Backpressure loop: a real-time producer would drop instead;
                // here we assert lossless FIFO transfer under retry.
                while producer.push(i).is_err() {
                    thread::sleep(Duration::from_micros(10));
                }
            }
        });
        for expected in 0..COUNT {
            let mut value = consumer.pop();
            while value.is_none() {
                value = consumer.pop();
            }
            assert_eq!(value, Some(expected));
        }
        handle.join().unwrap();
        assert!(consumer.is_empty());
    }

    #[test]
    fn drop_releases_unconsumed_values() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Counted(#[allow(dead_code)] u32);
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        DROPS.store(0, Ordering::SeqCst);
        {
            let ring = SpscRing::with_capacity(4);
            for i in 0..3u32 {
                let _ = ring.push(Counted(i));
            }
        }
        // UnsafeCell<Option<T>> drops its contents with the ring.
        assert_eq!(DROPS.load(Ordering::SeqCst), 3);
    }
}
