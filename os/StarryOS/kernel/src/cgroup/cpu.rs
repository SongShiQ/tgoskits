//! cgroup v2 cpu controller.
//!
//! Provides file interfaces for cpu.weight and cpu.max.
//! Implements bandwidth throttling via tick hook.

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::task::AsThread;

/// Per-cgroup cpu.max bandwidth state.
pub struct BandwidthState {
    pub quota: AtomicI64,
    pub period: AtomicI64,
    pub consumed: AtomicI64,
    pub nr_periods: AtomicU64,
    pub nr_throttled: AtomicU64,
    pub throttled_usec: AtomicU64,
    pub period_start: AtomicU64,
}

impl BandwidthState {
    pub fn new() -> Self {
        Self {
            quota: AtomicI64::new(-1),
            period: AtomicI64::new(100_000),
            consumed: AtomicI64::new(0),
            nr_periods: AtomicU64::new(0),
            nr_throttled: AtomicU64::new(0),
            throttled_usec: AtomicU64::new(0),
            period_start: AtomicU64::new(0),
        }
    }

    /// Check if the cgroup has a quota set.
    pub fn has_quota(&self) -> bool {
        self.quota.load(Ordering::Acquire) >= 0
    }

    /// Check if the cgroup is throttled (consumed >= quota).
    pub fn is_throttled(&self) -> bool {
        let quota = self.quota.load(Ordering::Acquire);
        if quota < 0 {
            return false; // unlimited
        }
        let consumed = self.consumed.load(Ordering::Acquire);
        consumed >= quota
    }

    /// Consume CPU time (in microseconds).
    /// Returns true if the cgroup is now throttled.
    pub fn consume(&self, usec: i64) -> bool {
        let quota = self.quota.load(Ordering::Acquire);
        if quota < 0 {
            return false; // unlimited
        }

        let consumed = self.consumed.fetch_add(usec, Ordering::AcqRel) + usec;
        if consumed >= quota {
            self.nr_throttled.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    /// Reset the period (call when period expires).
    pub fn reset_period(&self) {
        self.consumed.store(0, Ordering::Release);
        self.nr_periods.fetch_add(1, Ordering::AcqRel);
    }
}

pub struct CpuState {
    pub cfs_quota: AtomicI64,
    pub cfs_period: AtomicI64,
    pub weight: AtomicI64,
    pub bandwidth: BandwidthState,
}

impl CpuState {
    pub fn new() -> Self {
        Self {
            cfs_quota: AtomicI64::new(-1),
            cfs_period: AtomicI64::new(100_000),
            weight: AtomicI64::new(100),
            bandwidth: BandwidthState::new(),
        }
    }
}

/// Tick hook for cgroup bandwidth accounting.
/// Called on each scheduler timer tick.
/// Increments consumed CPU time and checks quota limits.
pub fn bandwidth_tick() {
    // Get the current task's cgroup
    // Note: This is called from scheduler_timer_tick which runs in IRQ context.
    // During initialization or idle task, there might not be a valid current task.
    let curr = match ax_task::current_may_uninit() {
        Some(task) => task,
        None => return, // No current task (shouldn't happen in normal operation)
    };

    // Skip idle tasks (named "idle")
    if curr.name() == "idle" {
        return;
    }

    let proc_data = match curr.try_as_thread() {
        Some(thr) => thr.proc_data.clone(),
        None => return, // Not a thread (shouldn't happen)
    };

    let cgroup = proc_data.cgroup.read().clone();

    // Check if this cgroup has a quota set
    if !cgroup.cpu.bandwidth.has_quota() {
        return; // unlimited, nothing to do
    }

    // Check if period has expired and reset if needed
    let now = ax_hal::time::monotonic_time_nanos() / 1000; // convert to microseconds
    let period_start = cgroup.cpu.bandwidth.period_start.load(Ordering::Acquire);
    let period = cgroup.cpu.bandwidth.period.load(Ordering::Acquire);

    if now - period_start >= period as u64 {
        // Period expired, reset consumed counter
        cgroup.cpu.bandwidth.reset_period();
        cgroup
            .cpu
            .bandwidth
            .period_start
            .store(now, Ordering::Release);

        // Unthrottle the task
        curr.set_throttled(false);
    }

    // Consume 1 tick worth of CPU time (typically 1ms = 1000usec)
    // TODO: Use actual tick duration from timer
    let tick_usec = 1000;
    let throttled = cgroup.cpu.bandwidth.consume(tick_usec);

    if throttled {
        // Mark the current task as throttled
        curr.set_throttled(true);
    }
}
