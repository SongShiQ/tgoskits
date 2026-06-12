//! cgroup v2 pids controller.
//!
//! Limits the number of processes in a cgroup.

use core::sync::atomic::{AtomicI64, Ordering};

/// Per-cgroup pids state.
pub struct PidsState {
    /// Current number of processes.
    pub current: AtomicI64,
    /// Maximum allowed (-1 = unlimited).
    pub max: AtomicI64,
}

impl PidsState {
    pub fn new() -> Self {
        Self {
            current: AtomicI64::new(0),
            max: AtomicI64::new(-1),
        }
    }

    /// Atomically check if a new process can be created and increment the counter.
    ///
    /// This uses a CAS loop to eliminate the TOCTOU race between `can_fork()`
    /// and `fork()` on SMP systems where two CPUs could both pass the check
    /// and exceed `pids.max`.
    ///
    /// Returns `true` if the fork was allowed (counter incremented),
    /// `false` if the limit would be exceeded.
    pub fn try_fork(&self) -> bool {
        let max = self.max.load(Ordering::Acquire);
        if max < 0 {
            // Unlimited: just increment
            self.current.fetch_add(1, Ordering::AcqRel);
            return true;
        }
        loop {
            let current = self.current.load(Ordering::Acquire);
            if current >= max {
                return false;
            }
            if self
                .current
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
            // CAS failed, retry
        }
    }

    /// Called when a process exits.
    pub fn exit(&self) {
        self.current.fetch_sub(1, Ordering::AcqRel);
    }
}
