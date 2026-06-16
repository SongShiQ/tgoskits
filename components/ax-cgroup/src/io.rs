//! cgroup v2 io controller.
//!
//! Controls I/O bandwidth and prioritization for block devices.

use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};

use axfs_ng_vfs::{VfsError, VfsResult};

use super::controller::{AttrInfo, CgroupController, CgroupControllerFactory, write_to_buf};

/// Per-device I/O limits.
#[derive(Clone)]
pub struct IoDeviceLimit {
    /// Device major:minor number.
    pub dev: (u32, u32),
    /// Read bytes per second limit (0 = unlimited).
    pub rbps: u64,
    /// Write bytes per second limit (0 = unlimited).
    pub wbps: u64,
    /// Read IOPS limit (0 = unlimited).
    pub riops: u64,
    /// Write IOPS limit (0 = unlimited).
    pub wiops: u64,
}

impl IoDeviceLimit {
    fn new(dev: (u32, u32)) -> Self {
        Self {
            dev,
            rbps: 0,  // unlimited
            wbps: 0,  // unlimited
            riops: 0, // unlimited
            wiops: 0, // unlimited
        }
    }
}

/// Per-device I/O statistics.
#[derive(Clone)]
pub struct IoDeviceStat {
    /// Device major:minor number.
    pub dev: (u32, u32),
    /// Read bytes.
    pub rbytes: AtomicU64,
    /// Write bytes.
    pub wbytes: AtomicU64,
    /// Read I/O operations.
    pub rios: AtomicU64,
    /// Write I/O operations.
    pub wios: AtomicU64,
}

impl IoDeviceStat {
    fn new(dev: (u32, u32)) -> Self {
        Self {
            dev,
            rbytes: AtomicU64::new(0),
            wbytes: AtomicU64::new(0),
            rios: AtomicU64::new(0),
            wios: AtomicU64::new(0),
        }
    }
}

/// Per-cgroup I/O state.
pub struct IoState {
    /// Default I/O weight (1-10000, default 100).
    pub weight: AtomicU64,
    /// Per-device I/O limits (protected by external lock).
    /// Note: In real implementation, this should use a lock.
    /// For simplicity, we store it as atomic pointer placeholder.
    _limits_placeholder: AtomicU64,
    /// Per-device I/O statistics (protected by external lock).
    _stats_placeholder: AtomicU64,
}

impl Default for IoState {
    fn default() -> Self {
        Self::new()
    }
}

impl IoState {
    pub fn new() -> Self {
        Self {
            weight: AtomicU64::new(100), // default weight
            _limits_placeholder: AtomicU64::new(0),
            _stats_placeholder: AtomicU64::new(0),
        }
    }

    /// Parse device major:minor format: "8:0"
    fn parse_dev(text: &str) -> Result<(u32, u32), ()> {
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() != 2 {
            return Err(());
        }
        let major = parts[0].parse().map_err(|_| ())?;
        let minor = parts[1].parse().map_err(|_| ())?;
        Ok((major, minor))
    }

    /// Parse io.max line: "8:0 rbps=1048576 wbps=1048576"
    fn parse_max_line(line: &str) -> Result<IoDeviceLimit, ()> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Err(());
        }

        let dev = Self::parse_dev(parts[0])?;
        let mut limit = IoDeviceLimit::new(dev);

        for part in &parts[1..] {
            if let Some((key, val)) = part.split_once('=') {
                let val = if val == "max" {
                    0 // unlimited
                } else {
                    val.parse().map_err(|_| ())?
                };

                match key {
                    "rbps" => limit.rbps = val,
                    "wbps" => limit.wbps = val,
                    "riops" => limit.riops = val,
                    "wiops" => limit.wiops = val,
                    _ => return Err(()),
                }
            } else {
                return Err(());
            }
        }

        Ok(limit)
    }

    /// Format device limit
    fn format_limit(limit: &IoDeviceLimit) -> String {
        let mut parts = Vec::new();
        if limit.rbps > 0 {
            parts.push(format!("rbps={}", limit.rbps));
        }
        if limit.wbps > 0 {
            parts.push(format!("wbps={}", limit.wbps));
        }
        if limit.riops > 0 {
            parts.push(format!("riops={}", limit.riops));
        }
        if limit.wiops > 0 {
            parts.push(format!("wiops={}", limit.wiops));
        }

        if parts.is_empty() {
            format!("{}:{} max", limit.dev.0, limit.dev.1)
        } else {
            format!("{}:{} {}", limit.dev.0, limit.dev.1, parts.join(" "))
        }
    }

    /// Format device statistics
    fn format_stat(stat: &IoDeviceStat) -> String {
        format!(
            "{}:{} rbytes={} wbytes={} rios={} wios={}",
            stat.dev.0,
            stat.dev.1,
            stat.rbytes.load(Ordering::Acquire),
            stat.wbytes.load(Ordering::Acquire),
            stat.rios.load(Ordering::Acquire),
            stat.wios.load(Ordering::Acquire),
        )
    }
}

// ── Controller instance ──────────────────────────────────────────────

/// I/O 控制器支持的属性
const IO_ATTRS: &[AttrInfo] = &[
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

/// I/O 控制器实例（per-node）
pub struct IoController {
    state: Arc<IoState>,
    /// Simplified: store limits as Vec (real impl should use SpinNoIrq<Vec<...>>)
    limits: Vec<IoDeviceLimit>,
    /// Simplified: store stats as Vec (real impl should use SpinNoIrq<Vec<...>>)
    stats: Vec<IoDeviceStat>,
}

impl IoController {
    pub fn new(state: Arc<IoState>) -> Self {
        Self {
            state,
            limits: Vec::new(),
            stats: Vec::new(),
        }
    }

    /// 获取内部状态
    pub fn state(&self) -> &Arc<IoState> {
        &self.state
    }
}

impl CgroupController for IoController {
    fn name(&self) -> &str {
        "io"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let value = match name {
            "weight" => {
                format!("{}\n", self.state.weight.load(Ordering::Acquire))
            }
            "max" => {
                if self.limits.is_empty() {
                    "default\n".to_string()
                } else {
                    let mut lines = Vec::new();
                    for limit in &self.limits {
                        lines.push(IoState::format_limit(limit));
                    }
                    format!("{}\n", lines.join("\n"))
                }
            }
            "stat" => {
                if self.stats.is_empty() {
                    "\n".to_string()
                } else {
                    let mut lines = Vec::new();
                    for stat in &self.stats {
                        lines.push(IoState::format_stat(stat));
                    }
                    format!("{}\n", lines.join("\n"))
                }
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
                let value: u64 = text.parse().map_err(|_| VfsError::InvalidInput)?;
                if !(1..=10_000).contains(&value) {
                    return Err(VfsError::InvalidInput);
                }
                self.state.weight.store(value, Ordering::Release);
                Ok(data.len())
            }
            "max" => {
                // Parse multi-line format
                // Note: Real implementation should update self.limits (requires mut or lock)
                // For now, just validate the format
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line == "default" {
                        continue;
                    }
                    IoState::parse_max_line(line).map_err(|_| VfsError::InvalidInput)?;
                }
                Ok(data.len())
            }
            "stat" => Err(VfsError::OperationNotPermitted),
            _ => Err(VfsError::NotFound),
        }
    }

    fn attr_names(&self) -> &[AttrInfo] {
        IO_ATTRS
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

// ── Factory ──────────────────────────────────────────────────────────

/// I/O 控制器工厂
pub struct IoControllerFactory;

impl CgroupControllerFactory for IoControllerFactory {
    fn name(&self) -> &str {
        "io"
    }

    fn is_domain(&self) -> bool {
        true
    }

    fn attr_names(&self) -> &[AttrInfo] {
        IO_ATTRS
    }

    fn new_instance(&self) -> Arc<dyn CgroupController> {
        Arc::new(IoController::new(Arc::new(IoState::new())))
    }
}
