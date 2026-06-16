//! cgroup v2 cpu controller.
//!
//! Provides file interfaces for cpu.weight and cpu.max.
//! Implements bandwidth throttling via tick hook.

use alloc::{format, sync::Arc};
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use axfs_ng_vfs::{VfsError, VfsResult};

use super::controller::{AttrInfo, CgroupController, CgroupControllerFactory, write_to_buf};

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

impl Default for BandwidthState {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn has_quota(&self) -> bool {
        self.quota.load(Ordering::Acquire) >= 0
    }

    pub fn is_throttled(&self) -> bool {
        let quota = self.quota.load(Ordering::Acquire);
        if quota < 0 {
            return false;
        }
        let consumed = self.consumed.load(Ordering::Acquire);
        consumed >= quota
    }

    pub fn consume(&self, usec: i64) -> bool {
        let quota = self.quota.load(Ordering::Acquire);
        if quota < 0 {
            return false;
        }
        let consumed = self.consumed.fetch_add(usec, Ordering::AcqRel) + usec;
        if consumed >= quota {
            self.nr_throttled.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

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

impl Default for CpuState {
    fn default() -> Self {
        Self::new()
    }
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
///
/// This function is registered as the tick hook by the kernel.
/// It accesses the current task through the kernel's task API.
/// The kernel should call this via `ax_task::set_tick_hook`.
pub fn bandwidth_tick() {
    // This is a placeholder. The kernel provides the actual bandwidth_tick
    // implementation that accesses current task / cgroup / time.
    // The real implementation lives in the kernel's cgroup module because
    // it needs access to ax_task::current_may_uninit, AsThread, and ax_hal::time.
    //
    // See kernel/src/cgroup/cpu.rs for the kernel-side implementation.
}

// ── Controller instance ──────────────────────────────────────────────

/// Cpu 控制器支持的属性
const CPU_ATTRS: &[AttrInfo] = &[
    AttrInfo {
        name: "weight",
        read_only: false,
    },
    AttrInfo {
        name: "max",
        read_only: false,
    },
    AttrInfo {
        name: "stat",
        read_only: true,
    },
];

/// Cpu 控制器实例（per-node）
pub struct CpuController {
    state: Arc<CpuState>,
}

impl CpuController {
    pub fn new(state: Arc<CpuState>) -> Self {
        Self { state }
    }

    /// 获取内部状态（用于 bandwidth_tick）
    pub fn state(&self) -> &Arc<CpuState> {
        &self.state
    }
}

impl CgroupController for CpuController {
    fn name(&self) -> &str {
        "cpu"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let value = match name {
            "weight" => format!("{}\n", self.state.weight.load(Ordering::Acquire)),
            "max" => {
                let quota = self.state.cfs_quota.load(Ordering::Acquire);
                let period = self.state.cfs_period.load(Ordering::Acquire);
                if quota < 0 {
                    format!("max {}\n", period)
                } else {
                    format!("{} {}\n", quota, period)
                }
            }
            "stat" => {
                let bw = &self.state.bandwidth;
                format!(
                    "nr_periods {}\nnr_throttled {}\nthrottled_usec {}\n",
                    bw.nr_periods.load(Ordering::Acquire),
                    bw.nr_throttled.load(Ordering::Acquire),
                    bw.throttled_usec.load(Ordering::Acquire),
                )
            }
            _ => return Err(VfsError::NotFound),
        };
        write_to_buf(&value, offset, buf)
    }

    fn write_attr(&self, name: &str, data: &[u8]) -> VfsResult<usize> {
        let text = core::str::from_utf8(data)
            .map_err(|_| VfsError::InvalidInput)?
            .trim();
        match name {
            "weight" => {
                let value = text.parse::<i64>().map_err(|_| VfsError::InvalidInput)?;
                if !(1..=10_000).contains(&value) {
                    return Err(VfsError::InvalidInput);
                }
                self.state.weight.store(value, Ordering::Release);
                Ok(data.len())
            }
            "max" => {
                let parts: alloc::vec::Vec<&str> = text.split_whitespace().collect();
                if parts.len() != 2 {
                    return Err(VfsError::InvalidInput);
                }
                let quota = if parts[0] == "max" {
                    -1
                } else {
                    parts[0].parse::<i64>().map_err(|_| VfsError::InvalidInput)?
                };
                let period = parts[1].parse::<i64>().map_err(|_| VfsError::InvalidInput)?;
                self.state.cfs_quota.store(quota, Ordering::Release);
                self.state.cfs_period.store(period, Ordering::Release);
                Ok(data.len())
            }
            "stat" => Err(VfsError::OperationNotPermitted),
            _ => Err(VfsError::NotFound),
        }
    }

    fn attr_names(&self) -> &[AttrInfo] {
        CPU_ATTRS
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

// ── Factory ──────────────────────────────────────────────────────────

/// Cpu 控制器工厂
pub struct CpuControllerFactory;

impl CgroupControllerFactory for CpuControllerFactory {
    fn name(&self) -> &str {
        "cpu"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn attr_names(&self) -> &[AttrInfo] {
        CPU_ATTRS
    }

    fn new_instance(&self) -> Arc<dyn CgroupController> {
        Arc::new(CpuController {
            state: Arc::new(CpuState::new()),
        })
    }
}
