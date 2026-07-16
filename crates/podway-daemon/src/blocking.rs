//! Daemon-wide synchronous admission control for blocking operations.
//!
//! The executor owns only a permit count. It never queues work on helper threads, and it releases
//! its internal mutex before invoking admitted work.

use std::{
    error::Error,
    fmt,
    num::NonZeroUsize,
    sync::{Condvar, Mutex},
};

/// A blocking operation that consumes the daemon-wide blocking-work budget.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BlockingOperationV1 {
    GitResolve,
    StoreInspect,
    StoreOpen,
    StoreRead,
    StoreWrite,
    ConfigRead,
    ProcedurePrepare,
    ArtifactHash,
    RegistryIo,
}

/// An error raised while acquiring or inspecting the blocking-work budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockingExecutorErrorV1 {
    /// The executor's internal mutex was poisoned by an unexpected panic.
    Poisoned,
    /// The executor is shutting down and no longer admits new work.
    Shutdown,
}

impl fmt::Display for BlockingExecutorErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Poisoned => formatter.write_str("blocking executor mutex was poisoned"),
            Self::Shutdown => formatter.write_str("blocking executor is shut down"),
        }
    }
}

impl Error for BlockingExecutorErrorV1 {}

/// A synchronous daemon-wide budget for filesystem, Git, SQLite, configuration, and hashing work.
pub struct BlockingExecutorV1 {
    capacity: NonZeroUsize,
    state: Mutex<BlockingExecutorStateV1>,
    permits_available: Condvar,
}

#[derive(Debug)]
struct BlockingExecutorStateV1 {
    active: usize,
    shutdown: bool,
}

impl BlockingExecutorV1 {
    /// Creates an executor with a nonzero maximum number of active blocking operations.
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            capacity,
            state: Mutex::new(BlockingExecutorStateV1 {
                active: 0,
                shutdown: false,
            }),
            permits_available: Condvar::new(),
        }
    }

    /// Returns the maximum number of operations that can be active concurrently.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity.get()
    }

    /// Returns the current number of admitted operations.
    ///
    /// This is a snapshot for observation, not a reservation.
    pub fn active(&self) -> Result<usize, BlockingExecutorErrorV1> {
        Ok(self.lock_state()?.active)
    }

    /// Returns whether the executor rejects newly submitted work.
    pub fn is_shutdown(&self) -> Result<bool, BlockingExecutorErrorV1> {
        Ok(self.lock_state()?.shutdown)
    }

    /// Acquires one daemon-wide blocking-work permit.
    ///
    /// The returned permit releases automatically when dropped. A shutdown wakes blocked callers
    /// and causes them to return [`BlockingExecutorErrorV1::Shutdown`].
    pub fn acquire(
        &self,
        operation: BlockingOperationV1,
    ) -> Result<BlockingPermitV1<'_>, BlockingExecutorErrorV1> {
        let mut state = self.lock_state()?;

        loop {
            if state.shutdown {
                return Err(BlockingExecutorErrorV1::Shutdown);
            }

            if state.active < self.capacity.get() {
                state.active += 1;
                return Ok(BlockingPermitV1 {
                    executor: self,
                    operation,
                });
            }

            state = self
                .permits_available
                .wait(state)
                .map_err(|_| BlockingExecutorErrorV1::Poisoned)?;
        }
    }

    /// Acquires a permit, invokes `work` synchronously, and releases the permit on every exit path.
    ///
    /// `work` runs without the executor mutex held. Its return value is preserved, so a fallible
    /// closure naturally returns `Result<Result<T, E>, BlockingExecutorErrorV1>`.
    pub fn run<T>(
        &self,
        operation: BlockingOperationV1,
        work: impl FnOnce() -> T,
    ) -> Result<T, BlockingExecutorErrorV1> {
        let _permit = self.acquire(operation)?;
        Ok(work())
    }

    /// Rejects future acquisitions and wakes all callers currently waiting for a permit.
    ///
    /// Permits already acquired remain valid until their holders release them.
    pub fn shutdown(&self) -> Result<(), BlockingExecutorErrorV1> {
        let mut state = self.lock_state()?;
        state.shutdown = true;
        drop(state);
        self.permits_available.notify_all();
        Ok(())
    }

    fn lock_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BlockingExecutorStateV1>, BlockingExecutorErrorV1> {
        self.state
            .lock()
            .map_err(|_| BlockingExecutorErrorV1::Poisoned)
    }
}

/// A borrowed blocking-work permit returned by [`BlockingExecutorV1::acquire`].
#[must_use = "dropping the permit releases its blocking-work capacity"]
pub struct BlockingPermitV1<'executor> {
    executor: &'executor BlockingExecutorV1,
    operation: BlockingOperationV1,
}

impl BlockingPermitV1<'_> {
    /// Returns the operation class that acquired this permit.
    #[must_use]
    pub const fn operation(&self) -> BlockingOperationV1 {
        self.operation
    }
}

impl Drop for BlockingPermitV1<'_> {
    fn drop(&mut self) {
        let mut state = match self.executor.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };

        debug_assert!(
            state.active > 0,
            "every permit increments the active count once"
        );
        if state.active > 0 {
            state.active -= 1;
            drop(state);
            self.executor.permits_available.notify_one();
        }
    }
}
