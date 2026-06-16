//! cgroup v2 memory controller.
//!
//! Limits and tracks memory usage for processes in a cgroup.

use alloc::{format, string::String, sync::Arc};
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use axfs_ng_vfs::{VfsError, VfsResult};

use super::controller::{AttrInfo, CgroupController, CgroupControllerFactory, write_to_buf};

/// Per-cgroup memory state.
pub struct MemoryState {
    /// Current memory usage in bytes.
    pub current: AtomicU64,
    /// Maximum allowed memory in bytes (-1 = unlimited).
    pub max: AtomicI64,
    /// High watermark for memory reclaim pressure (bytes, -1 = unlimited).
    pub high: AtomicI64,
    /// Memory protection threshold (bytes, 0 = no protection).
    pub low: AtomicI64,
    /// Hard memory protection (bytes, 0 = no protection).
    pub min: AtomicI64,
    /// Number of times the limit was hit.
    pub events_max: AtomicU64,
    /// Number of times high watermark was exceeded.
    pub events_high: AtomicU64,
    /// Number of OOM kills.
    pub events_oom: AtomicU64,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryState {
    pub fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
            max: AtomicI64::new(-1),      // unlimited
            high: AtomicI64::new(-1),     // unlimited
            low: AtomicI64::new(0),       // no protection
            min: AtomicI64::new(0),       // no protection
            events_max: AtomicU64::new(0),
            events_high: AtomicU64::new(0),
            events_oom: AtomicU64::new(0),
        }
    }

    /// Parse memory value: "1G", "512M", "1048576" (bytes), "max"
    fn parse_memory_value(text: &str) -> Result<i64, ()> {
        let text = text.trim();
        if text == "max" {
            return Ok(-1);
        }

        // Handle size suffixes: K, M, G, T
        let (num_part, multiplier) = if text.ends_with('K') || text.ends_with('k') {
            (&text[..text.len() - 1], 1024i64)
        } else if text.ends_with('M') || text.ends_with('m') {
            (&text[..text.len() - 1], 1024i64 * 1024)
        } else if text.ends_with('G') || text.ends_with('g') {
            (&text[..text.len() - 1], 1024i64 * 1024 * 1024)
        } else if text.ends_with('T') || text.ends_with('t') {
            (&text[..text.len() - 1], 1024i64 * 1024 * 1024 * 1024)
        } else {
            (text, 1i64)
        };

        let num: i64 = num_part.parse().map_err(|_| ())?;
        if num < 0 {
            return Err(());
        }
        num.checked_mul(multiplier).ok_or(())
    }

    /// Format memory value: bytes to human-readable string
    fn format_memory_value(bytes: i64) -> String {
        if bytes < 0 {
            return "max".to_string();
        }
        format!("{}", bytes)
    }

    /// Check if allocation would exceed limit
    pub fn can_charge(&self, bytes: u64) -> bool {
        let max = self.max.load(Ordering::Acquire);
        if max < 0 {
            return true; // unlimited
        }
        let current = self.current.load(Ordering::Acquire);
        current.saturating_add(bytes) <= max as u64
    }

    /// Charge memory usage (does not enforce limit, caller must check)
    pub fn charge(&self, bytes: u64) {
        self.current.fetch_add(bytes, Ordering::AcqRel);
    }

    /// Uncharge memory usage
    pub fn uncharge(&self, bytes: u64) {
        loop {
            let current = self.current.load(Ordering::Acquire);
            let new_val = current.saturating_sub(bytes);
            if self
                .current
                .compare_exchange(current, new_val, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }
}

// ── Controller instance ──────────────────────────────────────────────

/// Memory 控制器支持的属性
const MEMORY_ATTRS: &[AttrInfo] = &[
    AttrInfo {
        name: "current",
        read_only: true,
    },
    AttrInfo {
        name: "max",
        read_only: false,
    },
    AttrInfo {
        name: "high",
        read_only: false,
    },
    AttrInfo {
        name: "low",
        read_only: false,
    },
    AttrInfo {
        name: "min",
        read_only: false,
    },
    AttrInfo {
        name: "events",
        read_only: true,
    },
];

/// Memory 控制器实例（per-node）
pub struct MemoryController {
    state: Arc<MemoryState>,
}

impl MemoryController {
    pub fn new(state: Arc<MemoryState>) -> Self {
        Self { state }
    }

    /// 获取内部状态（用于内存计费）
    pub fn state(&self) -> &Arc<MemoryState> {
        &self.state
    }
}

impl CgroupController for MemoryController {
    fn name(&self) -> &str {
        "memory"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let value = match name {
            "current" => {
                format!("{}\n", self.state.current.load(Ordering::Acquire))
            }
            "max" => {
                let max = self.state.max.load(Ordering::Acquire);
                format!("{}\n", MemoryState::format_memory_value(max))
            }
            "high" => {
                let high = self.state.high.load(Ordering::Acquire);
                format!("{}\n", MemoryState::format_memory_value(high))
            }
            "low" => {
                format!("{}\n", self.state.low.load(Ordering::Acquire))
            }
            "min" => {
                format!("{}\n", self.state.min.load(Ordering::Acquire))
            }
            "events" => {
                format!(
                    "max {}\nhigh {}\noom {}\n",
                    self.state.events_max.load(Ordering::Acquire),
                    self.state.events_high.load(Ordering::Acquire),
                    self.state.events_oom.load(Ordering::Acquire),
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
            "max" => {
                let value = MemoryState::parse_memory_value(text)
                    .map_err(|_| VfsError::InvalidInput)?;
                self.state.max.store(value, Ordering::Release);
                Ok(data.len())
            }
            "high" => {
                let value = MemoryState::parse_memory_value(text)
                    .map_err(|_| VfsError::InvalidInput)?;
                self.state.high.store(value, Ordering::Release);
                Ok(data.len())
            }
            "low" => {
                let value = MemoryState::parse_memory_value(text)
                    .map_err(|_| VfsError::InvalidInput)?;
                if value < 0 {
                    return Err(VfsError::InvalidInput);
                }
                self.state.low.store(value, Ordering::Release);
                Ok(data.len())
            }
            "min" => {
                let value = MemoryState::parse_memory_value(text)
                    .map_err(|_| VfsError::InvalidInput)?;
                if value < 0 {
                    return Err(VfsError::InvalidInput);
                }
                self.state.min.store(value, Ordering::Release);
                Ok(data.len())
            }
            "current" | "events" => Err(VfsError::OperationNotPermitted),
            _ => Err(VfsError::NotFound),
        }
    }

    fn attr_names(&self) -> &[AttrInfo] {
        MEMORY_ATTRS
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

// ── Factory ──────────────────────────────────────────────────────────

/// Memory 控制器工厂
pub struct MemoryControllerFactory;

impl CgroupControllerFactory for MemoryControllerFactory {
    fn name(&self) -> &str {
        "memory"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn attr_names(&self) -> &[AttrInfo] {
        MEMORY_ATTRS
    }

    fn new_instance(&self) -> Arc<dyn CgroupController> {
        Arc::new(MemoryController {
            state: Arc::new(MemoryState::new()),
        })
    }
}
