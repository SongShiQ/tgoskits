//! Pids controller — limits the number of tasks in a cgroup.
//!
//! Exposes control files:
//! - `pids.max` — maximum number of tasks (read/write, "max" for unlimited)
//! - `pids.current` — current number of tasks (read-only)
//! - `pids.peak` — high-water mark of task count (read-only)
//!
//! Design follows Linux cgroup v2 pids controller with hierarchy-aware
//! charging: when a task is charged to a child, the parent's count also
//! increments. Uncharging propagates upward on task exit.

use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use ax_errno::{AxError, AxResult};

use super::{Controller, SubControl, SubControlStatic, SubController};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Sentinel value meaning "no limit" (`pids.max` = "max").
const PIDS_MAX_UNLIMITED: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// PidsController
// ---------------------------------------------------------------------------

/// Per-cgroup pids controller state.
///
/// Tracks current, peak, and maximum allowed task count. All counters use
/// atomic operations for lock-free fast-path charging.
pub struct PidsController {
    /// Maximum number of tasks allowed. `u32::MAX` means unlimited.
    max_pids: AtomicU32,
    /// Current number of tasks charged to this cgroup.
    current_count: AtomicU32,
    /// High-water mark of `current_count`.
    peak_count: AtomicU32,
}

impl PidsController {
    // -- Public accessors ---------------------------------------------------

    /// Current number of tasks in this cgroup.
    pub fn current(&self) -> u32 {
        self.current_count.load(Ordering::Acquire)
    }

    /// Peak (high-water mark) of task count.
    pub fn peak(&self) -> u32 {
        self.peak_count.load(Ordering::Acquire)
    }

    /// Maximum allowed tasks. Returns `None` if unlimited.
    pub fn max(&self) -> Option<u32> {
        let m = self.max_pids.load(Ordering::Acquire);
        if m == PIDS_MAX_UNLIMITED {
            None
        } else {
            Some(m)
        }
    }

    /// Set the maximum allowed tasks. Pass `None` for unlimited.
    pub fn set_max(&self, max: Option<u32>) {
        let val = max.unwrap_or(PIDS_MAX_UNLIMITED);
        self.max_pids.store(val, Ordering::Release);
    }

    // -- Hierarchy-aware charging -------------------------------------------

    /// Try to charge one task to this cgroup and all ancestors.
    ///
    /// Uses CAS-based atomic charging at each level (try_charge_local).
    /// If any level fails, rolls back all previously charged levels.
    /// This prevents concurrent forks from exceeding pids.max.
    ///
    /// Returns `Ok(())` if the charge succeeds at all levels, or
    /// `Err(WouldBlock)` if any ancestor would exceed its limit.
    pub fn try_charge(controller: &SubController<Self>) -> AxResult<()> {
        // Collect the ancestor chain.
        let mut chain = alloc::vec::Vec::new();
        let mut current = controller;
        loop {
            chain.push(current);
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        // CAS charge each level. Rollback on failure.
        let mut charged = 0usize;
        for node in &chain {
            if let Some(inner) = node.inner() {
                if let Err(e) = inner.try_charge_local() {
                    // Rollback: uncharge all previously charged levels.
                    for rollback_node in &chain[..charged] {
                        if let Some(rollback_inner) = rollback_node.inner() {
                            rollback_inner.uncharge_local();
                        }
                    }
                    return Err(e);
                }
                charged += 1;
            }
        }

        Ok(())
    }

    /// Charge one task without checking limits (for root cgroup or when
    /// limits are being set after tasks already exist).
    pub fn charge(controller: &SubController<Self>) {
        let mut current = controller;
        loop {
            if let Some(inner) = current.inner() {
                inner.charge_local();
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }

    /// Uncharge one task from this cgroup and all ancestors.
    ///
    /// Called from `do_exit()` / task cleanup.
    pub fn uncharge(controller: &SubController<Self>) {
        let mut current = controller;
        loop {
            if let Some(inner) = current.inner() {
                inner.uncharge_local();
            }
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }

    // -- Local (single-node) operations -------------------------------------

    /// Try to charge locally using CAS loop (check-then-increment).
    ///
    /// This avoids the TOCTOU race of the previous fetch-add-then-check
    /// approach. The CAS loop ensures we never transiently exceed max.
    pub(crate) fn try_charge_local(&self) -> AxResult<()> {
        loop {
            let max = self.max_pids.load(Ordering::Acquire);
            let current = self.current_count.load(Ordering::Acquire);
            if max != PIDS_MAX_UNLIMITED && current >= max {
                return Err(AxError::WouldBlock);
            }
            match self.current_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.peak_count.fetch_max(current + 1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(_) => continue, // Retry on CAS failure
            }
        }
    }

    /// Charge locally without limit check.
    pub(crate) fn charge_local(&self) {
        let new = self.current_count.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak_count.fetch_max(new, Ordering::Relaxed);
    }

    /// Uncharge locally. Saturates at 0 to prevent underflow.
    pub(crate) fn uncharge_local(&self) {
        loop {
            let current = self.current_count.load(Ordering::Acquire);
            if current == 0 {
                break;
            }
            if self
                .current_count
                .compare_exchange_weak(
                    current,
                    current - 1,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SubControl impl
// ---------------------------------------------------------------------------

impl SubControl for PidsController {
    fn read_attr_at(&self, name: &str, offset: usize, buf: &mut [u8]) -> AxResult<usize> {
        let value = match name {
            "pids.max" => match self.max() {
                Some(m) => format!("{m}\n"),
                None => "max\n".to_string(),
            },
            "pids.current" => format!("{}\n", self.current()),
            "pids.peak" => format!("{}\n", self.peak()),
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
            "pids.max" => {
                let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?;
                let s = s.trim();
                if s == "max" {
                    self.set_max(None);
                } else {
                    let val: u32 = s.parse().map_err(|_| AxError::InvalidInput)?;
                    self.set_max(Some(val));
                }
                Ok(data.len())
            }
            "pids.current" | "pids.peak" => Err(AxError::PermissionDenied),
            _ => Err(AxError::NotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// SubControlStatic impl
// ---------------------------------------------------------------------------

impl SubControlStatic for PidsController {
    fn new(_is_root: bool, _is_active: bool) -> Self {
        Self {
            // Linux cgroup v2: default pids.max = "max" (unlimited) for ALL cgroups.
            // Only explicitly writing a value should limit.
            max_pids: AtomicU32::new(PIDS_MAX_UNLIMITED),
            current_count: AtomicU32::new(0),
            peak_count: AtomicU32::new(0),
        }
    }

    fn read_from(controller: &Controller) -> Arc<SubController<Self>> {
        controller.pids()
    }
}
