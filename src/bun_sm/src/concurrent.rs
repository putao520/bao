//! Concurrent task types.

/// Task that can be enqueued cross-thread.
pub trait ConcurrentTask: Send + 'static {
    fn run(self: Box<Self>);
}

/// Task that runs on a work pool thread.
pub trait WorkPoolTask: Send + 'static {
    fn run(self: Box<Self>);
}

/// Generic task type — union of all task kinds.
pub trait AnyTask: Send + 'static {
    fn run(self: Box<Self>);
}
