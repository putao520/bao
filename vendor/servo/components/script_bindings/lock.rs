/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::sync::OnceLock;

/// A OnceLock wrapping a type that is not considered threadsafe by the Rust compiler, but
/// will be used in a threadsafe manner (it will not be mutated, after being initialized).
///
/// This is needed to allow using JS API types (which usually involve raw pointers) in static initializers,
/// when Servo guarantees through the use of OnceLock that only one thread will ever initialize
/// the value.
pub struct ThreadUnsafeOnceLock<T>(OnceLock<T>);

impl<T> ThreadUnsafeOnceLock<T> {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// Initialize the value inside this lock.
    ///
    /// BAO PATCH (BCE-20260627-009): Idempotent — if the lock has already been
    /// initialized, this is a no-op instead of panicking. The original servo
    /// `assert!(self.0.set(val).is_ok())` panics on any re-init, which breaks
    /// bao's multi-BaoRuntime model: cargo's default multi-threaded test runner
    /// creates several `BaoRuntime` instances in the same process (one per test
    /// thread), each calling `Servo::new`, which triggers servo's codegen statics
    /// (`CLASS_OPS`, `Class`, `INTERFACE_OBJECT_CLASS`, `*_specs`, etc.) to run
    /// their `ThreadUnsafeOnceLock::set` initializers again. These statics always
    /// initialize from identical compile-time-constant data, so a re-init carries
    /// the same value and is safe to ignore. A panic here would abort the whole
    /// test binary.
    ///
    /// We intentionally do NOT compare old vs new (the type `T` is not required
    /// to be `PartialEq`, and these are `unsafe` JS-API types that must not be
    /// read casually). Re-init with the same static initializer data is the only
    /// legitimate path in bao's embedding model; the process-global `OnceLock`
    /// guarantees the first value wins and is never overwritten.
    pub fn set(&self, val: T) {
        let _ = self.0.set(val);
    }

    /// Get a reference to the value inside this lock. Panics if the lock has not been initialized.
    ///
    /// # Safety
    ///   The caller must ensure that it does not mutate value contained inside this lock
    ///   (using interior mutability).
    pub unsafe fn get(&self) -> &T {
        self.0.get().unwrap()
    }
}

unsafe impl<T> Sync for ThreadUnsafeOnceLock<T> {}
unsafe impl<T> Send for ThreadUnsafeOnceLock<T> {}
