//! Thread-safe reference wrapper.

use std::sync::Arc;

pub struct ThreadSafe<T: Send + 'static> {
    inner: Arc<T>,
}

impl<T: Send + 'static> ThreadSafe<T> {
    pub fn new(value: T) -> Self {
        ThreadSafe {
            inner: Arc::new(value),
        }
    }

    pub fn get(&self) -> &T {
        &self.inner
    }

    pub fn into_inner(self) -> Arc<T> {
        self.inner
    }

    pub fn clone_inner(&self) -> Arc<T> {
        Arc::clone(&self.inner)
    }
}

unsafe impl<T: Send + 'static> Send for ThreadSafe<T> {}
unsafe impl<T: Send + 'static> Sync for ThreadSafe<T> {}
