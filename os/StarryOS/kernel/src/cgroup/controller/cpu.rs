//! Cpu controller — CPU bandwidth and weight management.
//!
//! This controller provides:
//! - `cpu.stat` — always-readable statistics (usage_usec, user_usec, system_usec, etc.)
//! - `cpu.weight` — CFS weight (1-10000, default 100, maps from nice)
//! - `cpu.max` — bandwidth limit ("max $period_us" or "$quota_us $period_us")
//!
//! The controller has two states:
//! - **CpuStats**: always present, provides read-only statistics
//! - **CpuControl**: present only when the controller is active, provides weight/max
//!
//! Design follows Linux cgroup v2 cpu controller with StarryOS-specific
//! bandwidth tick integration.

use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use ax_errno::{AxError, AxResult};

use super::{Controller, SubControl, SubControlStatic, SubController};

const CPU_MAX_MIN_PERIOD_US: i64 = 1_000;
const CPU_MAX_MAX_PERIOD_US: i64 = 1_000_000;

// ---------------------------------------------------------------------------
// BandwidthState
// ---------------------------------------------------------------------------

/// Per-cgroup CPU bandwidth state.
///
/// Tracks CFS bandwidth quota, period, and consumption statistics.
/// Used by the scheduler's tick hook to enforce bandwidth limits.
pub struct BandwidthState {
    /// CFS quota in microseconds (-1 = unlimited).
    pub quota: AtomicI64,
    /// CFS period in microseconds (default 100ms = 100_000us).
    pub period: AtomicI64,
    /// Consumed time in current period (microseconds).
    pub consumed: AtomicI64,
    /// Total number of periods elapsed.
    pub nr_periods: AtomicU64,
    /// Number of periods where the cgroup was throttled.
    pub nr_throttled: AtomicU64,
    /// Total throttled time in microseconds.
    pub throttled_usec: AtomicU64,
    /// Start time of the current period (microseconds since boot).
    pub period_start: AtomicU64,
}

impl BandwidthState {
    pub fn new() -> Self {
        Self {
            quota: AtomicI64::new(-1),       // Unlimited
            period: AtomicI64::new(100_000),  // 100ms default
            consumed: AtomicI64::new(0),
            nr_periods: AtomicU64::new(0),
            nr_throttled: AtomicU64::new(0),
            throttled_usec: AtomicU64::new(0),
            period_start: AtomicU64::new(0),
        }
    }

    /// Reset the current period (called when period advances).
    pub fn reset_period(&self, now_us: u64) {
        self.consumed.store(0, Ordering::Relaxed);
        self.period_start.store(now_us, Ordering::Relaxed);
        self.nr_periods.fetch_add(1, Ordering::Relaxed);
    }

    /// Consume time in the current period.
    ///
    /// Returns `true` if the cgroup should be throttled.
    pub fn consume(&self, tick_usec: i64) -> bool {
        let quota = self.quota.load(Ordering::Relaxed);
        if quota < 0 {
            return false; // Unlimited
        }

        let consumed = self.consumed.fetch_add(tick_usec, Ordering::Relaxed) + tick_usec;
        if consumed >= quota {
            self.nr_throttled.fetch_add(1, Ordering::Relaxed);
            self.throttled_usec
                .fetch_add(tick_usec as u64, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// CpuController
// ---------------------------------------------------------------------------

/// Cpu controller implementation.
///
/// Combines always-readable statistics with optional bandwidth control.
pub struct CpuController {
    /// CPU weight (1-10000, default 100).
    pub weight: AtomicI64,
    /// Bandwidth state for cpu.max enforcement.
    pub bandwidth: BandwidthState,
}

impl CpuController {
    /// Get the current cpu.stat output.
    ///
    /// Format: "usage_usec $N\nuser_usec $N\nsystem_usec $N\nnr_periods $N\nnr_throttled $N\nthrottled_usec $N\n"
    fn stat_text(&self) -> alloc::string::String {
        format!(
            "usage_usec {}\nuser_usec {}\nsystem_usec {}\nnr_periods {}\nnr_throttled {}\nthrottled_usec {}\n",
            self.bandwidth.consumed.load(Ordering::Relaxed),
            0, // user_usec — not tracked separately yet
            0, // system_usec — not tracked separately yet
            self.bandwidth.nr_periods.load(Ordering::Relaxed),
            self.bandwidth.nr_throttled.load(Ordering::Relaxed),
            self.bandwidth.throttled_usec.load(Ordering::Relaxed),
        )
    }

    /// Get the current cpu.weight value.
    fn weight_text(&self) -> alloc::string::String {
        format!("{}\n", self.weight.load(Ordering::Relaxed))
    }

    /// Get the current cpu.max value.
    ///
    /// Format: "$quota $period" or "max $period"
    fn max_text(&self) -> alloc::string::String {
        let quota = self.bandwidth.quota.load(Ordering::Relaxed);
        let period = self.bandwidth.period.load(Ordering::Relaxed);
        if quota < 0 {
            format!("max {}\n", period)
        } else {
            format!("{} {}\n", quota, period)
        }
    }

    /// Parse and apply cpu.max value.
    ///
    /// Format: "$quota $period" or "max $period"
    fn apply_max(&self, data: &[u8]) -> AxResult<usize> {
        let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?;
        let s = s.trim();

        if s == "max" {
            self.bandwidth.quota.store(-1, Ordering::Relaxed);
            return Ok(data.len());
        }

        let parts: alloc::vec::Vec<&str> = s.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(AxError::InvalidInput);
        }

        let quota = if parts[0] == "max" {
            -1i64
        } else {
            let quota: i64 = parts[0].parse().map_err(|_| AxError::InvalidInput)?;
            if quota <= 0 {
                return Err(AxError::InvalidInput);
            }
            quota
        };
        let period: i64 = parts[1].parse().map_err(|_| AxError::InvalidInput)?;

        if !(CPU_MAX_MIN_PERIOD_US..=CPU_MAX_MAX_PERIOD_US).contains(&period) {
            return Err(AxError::InvalidInput);
        }

        self.bandwidth.quota.store(quota, Ordering::Relaxed);
        self.bandwidth.period.store(period, Ordering::Relaxed);
        Ok(data.len())
    }

    /// Parse and apply cpu.weight value.
    ///
    /// Range: 1-10000, default 100.
    fn apply_weight(&self, data: &[u8]) -> AxResult<usize> {
        let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?;
        let s = s.trim();
        let val: i64 = s.parse().map_err(|_| AxError::InvalidInput)?;

        if val < 1 || val > 10_000 {
            return Err(AxError::InvalidInput);
        }

        self.weight.store(val, Ordering::Relaxed);
        Ok(data.len())
    }
}

// ---------------------------------------------------------------------------
// SubControl impl
// ---------------------------------------------------------------------------

impl SubControl for CpuController {
    fn read_attr_at(&self, name: &str, offset: usize, buf: &mut [u8]) -> AxResult<usize> {
        let value = match name {
            "cpu.stat" => self.stat_text(),
            "cpu.weight" => self.weight_text(),
            "cpu.max" => self.max_text(),
            _ => return Err(AxError::NotFound),
        };

        let bytes = value.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let remaining = &bytes[offset..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        Ok(n)
    }

    fn write_attr(&self, name: &str, data: &[u8]) -> AxResult<usize> {
        match name {
            "cpu.stat" => Err(AxError::PermissionDenied), // Read-only
            "cpu.weight" => self.apply_weight(data),
            "cpu.max" => self.apply_max(data),
            _ => Err(AxError::NotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// SubControlStatic impl
// ---------------------------------------------------------------------------

impl SubControlStatic for CpuController {
    fn new(is_root: bool, _is_active: bool) -> Self {
        let _ = is_root;
        Self {
            weight: AtomicI64::new(100), // Default weight
            bandwidth: BandwidthState::new(),
        }
    }

    fn read_from(controller: &Controller) -> Arc<SubController<Self>> {
        controller.cpu()
    }
}

// ---------------------------------------------------------------------------
// bandwidth_tick — scheduler integration
// ---------------------------------------------------------------------------

/// Called on every scheduler timer tick to consume quota and throttle.
///
/// This function integrates with the scheduler to enforce CFS bandwidth
/// limits. It should be registered as a tick hook during cgroup init.
///
/// # Safety
/// This function is called from the scheduler tick handler, which runs
/// in interrupt context. All operations use atomic loads/stores.
pub fn bandwidth_tick() {
    // TODO: Integrate with task's cgroup association.
    //
    // The implementation should:
    // 1. Get the current task's cgroup
    // 2. Load the cgroup's BandwidthState
    // 3. Check if period has advanced (reset if so)
    // 4. Consume quota for this tick
    // 5. Throttle if quota exceeded
    //
    // This requires the process-to-cgroup mapping to be implemented
    // in ProcessData (Phase 3).
}

/// Get the monotonic time in microseconds.
fn _now_usec() -> u64 {
    // TODO: Use ax_runtime::hal::time::monotonic_time()
    0
}
